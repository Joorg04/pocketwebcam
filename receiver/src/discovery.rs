//! Finds phones advertising `_pocketwebcam._tcp` over mDNS.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use anyhow::Result;
use mdns_sd::{Receiver, ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::pairing::Fingerprint;
use crate::protocol::{PROTOCOL_VERSION, SERVICE_TYPE};

/// After a failed connection, the phone is left out of the list for this long while the
/// network is asked whether it's still there.
const RECHECK: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phone {
    pub name: String,
    pub addr: SocketAddr,
    /// The phone app's own identifier (TXT key `id`). Unlike the name, it doesn't change
    /// when the phone is renamed. Missing from older phone apps.
    pub id: Option<String>,
    /// Protocol version from TXT `v`. Phones with another version can't connect.
    pub version: u32,
    /// The phone's certificate fingerprint (TXT `fp`), to tell paired phones from others
    /// before connecting. The TLS handshake checks it again: a TXT record can be faked.
    pub fingerprint: Option<Fingerprint>,
    /// TXT `pair`: set while the phone holds a scanned pairing code (`Secret::tag`).
    pub pair_tag: Option<String>,
}

impl Phone {
    /// Speaks the same protocol version as this receiver.
    pub fn compatible(&self) -> bool {
        self.version == PROTOCOL_VERSION
    }
}

/// A running search that keeps a live list of the phones on the network.
///
/// One search runs for as long as the browser lives: phones show up as soon as they
/// announce themselves and leave the list when they say goodbye (or stop answering),
/// without asking the whole network again every few seconds.
pub struct Browser {
    daemon: ServiceDaemon,
    events: Receiver<ServiceEvent>,
    service_type: String,
    /// By full mDNS name, which is unique on a network.
    phones: BTreeMap<String, Phone>,
    /// Phones that just failed to connect, hidden until this time.
    rechecking: BTreeMap<String, Instant>,
}

impl Browser {
    pub fn start() -> Result<Browser> {
        Self::start_for(SERVICE_TYPE)
    }

    fn start_for(service_type: &str) -> Result<Browser> {
        let daemon = ServiceDaemon::new()?;
        let events = daemon.browse(service_type)?;
        Ok(Browser {
            daemon,
            events,
            service_type: service_type.to_string(),
            phones: BTreeMap::new(),
            rechecking: BTreeMap::new(),
        })
    }

    /// Waits up to `timeout` for news from the network and takes it in. An error means the
    /// search has died and a new browser is needed.
    pub fn poll(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) => self.handle(event),
                Err(e) if self.events.is_disconnected() => anyhow::bail!("phone discovery stopped: {e}"),
                Err(_) => return Ok(()), // nothing new within `timeout`
            }
        }
    }

    fn handle(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                let fullname = info.get_fullname().to_string();
                if let Some(phone) = phone_from(&info, &self.service_type) {
                    // Fresh news about a phone we were unsure of: it's there.
                    self.rechecking.remove(&fullname);
                    self.phones.insert(fullname, phone);
                }
            }
            ServiceEvent::ServiceRemoved(_, fullname) => {
                self.phones.remove(&fullname);
                self.rechecking.remove(&fullname);
            }
            _ => {}
        }
    }

    /// The phones on the network right now, sorted by name.
    pub fn phones(&self) -> Vec<Phone> {
        let now = Instant::now();
        let mut phones: Vec<Phone> = self
            .phones
            .iter()
            .filter(|(fullname, _)| self.rechecking.get(*fullname).is_none_or(|until| *until <= now))
            .map(|(_, phone)| phone.clone())
            .collect();
        phones.sort_by(|a, b| a.name.cmp(&b.name));
        phones
    }

    /// Call when connecting to `addr` failed. A phone that left without saying goodbye
    /// (Wi-Fi off, out of range) is still in the network's memory for a while; this asks
    /// whether it's really there and keeps it out of the list meanwhile.
    pub fn connection_failed(&mut self, addr: SocketAddr) {
        let until = Instant::now() + RECHECK;
        for (fullname, _) in self.phones.iter().filter(|(_, p)| p.addr == addr) {
            self.rechecking.insert(fullname.clone(), until);
            // No answer within RECHECK makes the daemon send ServiceRemoved.
            let _ = self.daemon.verify(fullname.clone(), RECHECK);
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

fn phone_from(info: &ServiceInfo, service_type: &str) -> Option<Phone> {
    // Prefer IPv4: link-local IPv6 needs a scope id that's easy to get wrong.
    let addrs = info.get_addresses();
    let ip = addrs.iter().find(|a| a.is_ipv4()).or_else(|| addrs.iter().next()).copied()?;
    let txt = |key: &str| info.get_property_val_str(key).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    Some(Phone {
        name: instance_name(info.get_fullname(), service_type),
        addr: SocketAddr::new(ip, info.get_port()),
        id: txt("id"),
        version: txt("v").and_then(|v| v.parse().ok()).unwrap_or(1),
        fingerprint: txt("fp").and_then(|fp| Fingerprint::parse(&fp).ok()),
        pair_tag: txt("pair"),
    })
}

/// Browses for `timeout` and returns every phone that resolved, sorted by name.
pub fn browse(timeout: Duration) -> Result<Vec<Phone>> {
    let mut browser = Browser::start()?;
    browser.poll(timeout)?;
    Ok(browser.phones())
}

/// Waits until a phone shows up (optionally one whose name contains `filter`).
pub fn find(filter: Option<&str>, timeout: Duration) -> Result<Option<Phone>> {
    let phones = browse(timeout)?;
    Ok(phones.into_iter().find(|p| match filter {
        Some(f) => p.name.to_lowercase().contains(&f.to_lowercase()),
        None => true,
    }))
}

/// `Nic's iPhone._pocketwebcam._tcp.local.` -> `Nic's iPhone`
fn instance_name(fullname: &str, service_type: &str) -> String {
    fullname
        .strip_suffix(service_type)
        .map(|s| s.trim_end_matches('.'))
        .unwrap_or(fullname)
        .replace("\\032", " ")
        .to_string()
}

/// Parses `ip`, `ip:port`, or `[v6]:port`, defaulting the port.
pub fn parse_host(host: &str, default_port: u16) -> Result<SocketAddr> {
    if let Ok(addr) = host.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let ip: IpAddr = host
        .parse()
        .map_err(|_| anyhow::anyhow!("not an IP address or IP:port: {host}"))?;
    Ok(SocketAddr::new(ip, default_port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_service_suffix() {
        assert_eq!(instance_name("Nic's iPhone._pocketwebcam._tcp.local.", SERVICE_TYPE), "Nic's iPhone");
    }

    #[test]
    fn parses_hosts() {
        assert_eq!(parse_host("10.0.0.5", 7310).unwrap().to_string(), "10.0.0.5:7310");
        assert_eq!(parse_host("10.0.0.5:9000", 7310).unwrap().port(), 9000);
        assert!(parse_host("phone.local", 7310).is_err());
    }

    /// Polls until `done` says the list is right, for at most `timeout`.
    fn wait_for(browser: &mut Browser, timeout: Duration, done: impl Fn(&[Phone]) -> bool) -> Vec<Phone> {
        let deadline = Instant::now() + timeout;
        loop {
            browser.poll(Duration::from_millis(100)).unwrap();
            let phones = browser.phones();
            if done(&phones) || Instant::now() >= deadline {
                return phones;
            }
        }
    }

    /// A phone on this computer, announced the way the apps do it, at this computer's own
    /// network addresses (mDNS doesn't run on the loopback interface).
    fn announce(daemon: &ServiceDaemon, service_type: &str, name: &str, port: u16, txt: &[(&str, &str)]) -> String {
        let host = format!("pw-test-{}-{port}.local.", std::process::id());
        let info = ServiceInfo::new(service_type, name, &host, "", port, txt).unwrap().enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon.register(info).unwrap();
        fullname
    }

    /// Uses real multicast on this computer's network, under a service type of its own so
    /// running copies of Pocket Webcam don't see it. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs a network that allows multicast"]
    fn sees_phones_come_and_go() {
        let service_type = format!("_pwtest{}._tcp.local.", std::process::id() % 10_000);
        let phone = ServiceDaemon::new().unwrap();
        let mut browser = Browser::start_for(&service_type).unwrap();
        assert!(browser.phones().is_empty());

        let fp = Fingerprint([5; 32]).to_string();
        let pixel = announce(&phone, &service_type, "Test Pixel", 47310, &[("v", "2"), ("id", "abc-123"), ("fp", &fp)]);
        announce(&phone, &service_type, "Old iPhone", 47311, &[("v", "1")]);
        let phones = wait_for(&mut browser, Duration::from_secs(8), |p| p.len() == 2);
        assert_eq!(phones.len(), 2, "both phones show up: {phones:?}");
        // Sorted by name; the id comes from the TXT record and is optional.
        assert_eq!((phones[0].name.as_str(), phones[0].id.as_deref()), ("Old iPhone", None));
        assert_eq!((phones[1].name.as_str(), phones[1].id.as_deref()), ("Test Pixel", Some("abc-123")));
        assert_eq!(phones[1].addr.port(), 47310);
        assert_eq!((phones[0].version, phones[0].fingerprint), (1, None));
        assert_eq!((phones[1].version, phones[1].fingerprint), (2, Some(Fingerprint([5; 32]))));
        assert!(phones[1].compatible() && !phones[0].compatible());

        // A failed connection hides the phone at once, and it comes back when it turns
        // out to be there after all.
        let addr = phones[1].addr;
        browser.connection_failed(addr);
        assert_eq!(browser.phones().len(), 1);
        let phones = wait_for(&mut browser, RECHECK + Duration::from_secs(3), |p| p.len() == 2);
        assert_eq!(phones.len(), 2, "a phone that still answers is listed again: {phones:?}");

        // Saying goodbye takes it off the list.
        let _ = phone.unregister(&pixel).unwrap().recv_timeout(Duration::from_secs(3));
        let phones = wait_for(&mut browser, Duration::from_secs(8), |p| p.len() == 1);
        assert_eq!(phones.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Old iPhone"]);
        let _ = phone.shutdown();
    }

    const VANISHING_TYPE: &str = "PW_TEST_VANISHING_TYPE";

    /// Not a test of its own: `forgets_a_phone_that_vanished` runs it in a second process,
    /// which announces a phone until it's killed.
    #[test]
    #[ignore = "helper for forgets_a_phone_that_vanished"]
    fn announce_until_killed() {
        let Ok(service_type) = std::env::var(VANISHING_TYPE) else { return };
        let daemon = ServiceDaemon::new().unwrap();
        announce(&daemon, &service_type, "Vanishing Phone", 47312, &[("v", "1")]);
        std::thread::sleep(Duration::from_secs(60));
    }

    /// A phone that drops off the network without a goodbye (Wi-Fi off, battery dead)
    /// stays in the network's memory; a failed connection gets it checked and removed.
    #[test]
    #[ignore = "needs a network that allows multicast"]
    fn forgets_a_phone_that_vanished() {
        let service_type = format!("_pwgone{}._tcp.local.", std::process::id() % 10_000);
        let mut browser = Browser::start_for(&service_type).unwrap();
        let mut phone = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "discovery::tests::announce_until_killed"])
            .env(VANISHING_TYPE, &service_type)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let phones = wait_for(&mut browser, Duration::from_secs(8), |p| p.len() == 1);
        phone.kill().unwrap(); // SIGKILL: no goodbye is sent
        let _ = phone.wait();
        assert_eq!(phones.len(), 1, "the other process's phone shows up");

        // Nothing has told the network that it's gone, so it's still listed...
        browser.poll(Duration::from_millis(500)).unwrap();
        assert_eq!(browser.phones().len(), 1);
        // ...until connecting to it fails.
        browser.connection_failed(phones[0].addr);
        assert!(browser.phones().is_empty(), "hidden while it's being checked");
        let started = Instant::now();
        while started.elapsed() < RECHECK + Duration::from_secs(4) {
            browser.poll(Duration::from_millis(200)).unwrap();
        }
        assert!(browser.phones().is_empty(), "a phone that doesn't answer stays off the list");
        assert!(browser.phones.is_empty(), "and the network's memory of it was dropped");
    }
}
