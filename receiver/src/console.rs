//! Interactive commands typed into the receiver's terminal.

use std::io::BufRead;
use std::sync::atomic::Ordering;
use std::thread;

use anyhow::{anyhow, bail, Result};

use crate::protocol::{Camera, ConfigUpdate};
use crate::session::{Current, Remote};

const HELP: &str = "\
commands:
  status                 connection, stream and phone info
  lens [n|name]          list lenses, or switch (e.g. `lens 2`, `lens tele`)
  res [WxH|4k|1080p|720p] list resolutions for this lens, or switch
  fps <n>                frame rate (e.g. `fps 60`)
  codec <h264|hevc>      video codec
  bitrate <mbps>         encoder bitrate (e.g. `bitrate 20`)
  zoom <x>               zoom factor (e.g. `zoom 2`)
  torch|mirror|stab <on|off>
  mic <on|off>           the phone's microphone
  key                    request a keyframe
  quit";

pub fn spawn(current: Current) {
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let words: Vec<&str> = line.split_whitespace().collect();
            let Some((&cmd, args)) = words.split_first() else { continue };
            match cmd {
                "help" | "?" => println!("{HELP}"),
                "quit" | "exit" | "q" => std::process::exit(0),
                _ => {
                    let remote = current.lock().unwrap().clone();
                    let result = match remote {
                        Some(r) => run(&r, cmd, args),
                        None => Err(anyhow!("no phone connected")),
                    };
                    if let Err(e) = result {
                        println!("{e}");
                    }
                }
            }
        }
    });
}

fn run(remote: &Remote, cmd: &str, args: &[&str]) -> Result<()> {
    let arg = args.first().copied();
    let mut update = ConfigUpdate::default();
    match (cmd, arg) {
        ("status", _) => return status(remote),
        ("key", _) => return remote.request_keyframe(),
        ("lens", None) => return list_lenses(remote),
        ("lens", Some(sel)) => update.camera = Some(pick_camera(remote, sel)?.id),
        ("res", None) => return list_resolutions(remote),
        ("res", Some(r)) => {
            let (w, h) = parse_resolution(r)?;
            update.width = Some(w);
            update.height = Some(h);
        }
        ("fps", Some(n)) => update.fps = Some(n.parse()?),
        ("codec", Some(c @ ("h264" | "hevc"))) => update.codec = Some(c.into()),
        ("bitrate", Some(m)) => update.bitrate = Some((m.parse::<f64>()? * 1_000_000.0) as u64),
        ("zoom", Some(z)) => update.zoom = Some(z.parse()?),
        ("torch", Some(v)) => update.torch = Some(parse_switch(v)?),
        ("mirror", Some(v)) => update.mirror = Some(parse_switch(v)?),
        ("stab", Some(v)) => update.stabilization = Some(parse_switch(v)?),
        ("mic", Some(v)) => update.audio = Some(parse_switch(v)?),
        _ => bail!("unknown command or missing argument; type `help`"),
    }
    remote.set_config(&update)
}

fn status(remote: &Remote) -> Result<()> {
    let info = remote.info.lock().unwrap();
    if let Some(h) = &info.hello {
        println!("phone:   {} ({} {})", h.name, h.app, h.app_version);
    }
    if let Some(s) = &info.state {
        let lens = info.caps.cameras.iter().find(|c| c.id == s.camera).map_or(s.camera.as_str(), |c| c.name.as_str());
        println!(
            "camera:  {lens}  {}x{} @ {} fps  {}  {:.1} Mbps  zoom {:.1}x",
            s.width, s.height, s.fps, s.codec, s.bitrate as f64 / 1e6, s.zoom
        );
        let battery = s.battery.map_or("?".into(), |b| format!("{:.0}%", b * 100.0));
        println!("device:  thermal {}  battery {battery}{}", s.thermal, if s.charging { " (charging)" } else { "" });
    }
    if info.caps.audio.is_some() {
        let mic = match &info.state {
            Some(s) if s.audio => "on".to_string(),
            Some(s) if s.mic_permission == "denied" => "off (permission denied on the phone)".to_string(),
            _ => "off".to_string(),
        };
        println!("mic:     {mic}  {} chunks received", remote.audio_chunks.load(Ordering::Relaxed));
    }
    if let Some(rtt) = info.rtt_ms {
        println!("network: {rtt:.1} ms round trip");
    }
    println!(
        "frames:  {} received, {} skipped waiting for keyframe",
        remote.frames.load(Ordering::Relaxed),
        remote.dropped.load(Ordering::Relaxed)
    );
    Ok(())
}

fn list_lenses(remote: &Remote) -> Result<()> {
    let info = remote.info.lock().unwrap();
    let active = info.state.as_ref().map(|s| s.camera.as_str());
    for (i, c) in info.caps.cameras.iter().enumerate() {
        let mark = if Some(c.id.as_str()) == active { "*" } else { " " };
        println!("{mark} {}  {:<12} {} ({})", i + 1, c.name, c.position, c.lens);
    }
    Ok(())
}

fn list_resolutions(remote: &Remote) -> Result<()> {
    let info = remote.info.lock().unwrap();
    let state = info.state.as_ref().ok_or_else(|| anyhow!("no state from phone yet"))?;
    let cam = info
        .caps
        .cameras
        .iter()
        .find(|c| c.id == state.camera)
        .ok_or_else(|| anyhow!("current lens not in capabilities"))?;
    for f in &cam.formats {
        let fps: Vec<String> = f.fps.iter().map(|n| n.to_string()).collect();
        let mark = if f.width == state.width && f.height == state.height { "*" } else { " " };
        println!("{mark} {}x{}  fps: {}", f.width, f.height, fps.join(", "));
    }
    Ok(())
}

/// Accepts a 1-based index, or a case-insensitive substring of the name or lens type.
fn pick_camera(remote: &Remote, sel: &str) -> Result<Camera> {
    let info = remote.info.lock().unwrap();
    let cams = &info.caps.cameras;
    if let Ok(n) = sel.parse::<usize>() {
        return cams.get(n.wrapping_sub(1)).cloned().ok_or_else(|| anyhow!("no lens #{n}"));
    }
    let sel = sel.to_lowercase();
    cams.iter()
        .find(|c| c.name.to_lowercase().contains(&sel) || c.lens.to_lowercase().contains(&sel))
        .cloned()
        .ok_or_else(|| anyhow!("no lens matching `{sel}`"))
}

pub fn parse_resolution(s: &str) -> Result<(u32, u32)> {
    match s.to_lowercase().as_str() {
        "4k" | "2160p" => return Ok((3840, 2160)),
        "1080p" => return Ok((1920, 1080)),
        "720p" => return Ok((1280, 720)),
        _ => {}
    }
    let (w, h) = s.split_once(['x', 'X']).ok_or_else(|| anyhow!("expected WxH, e.g. 1920x1080"))?;
    Ok((w.parse()?, h.parse()?))
}

fn parse_switch(s: &str) -> Result<bool> {
    match s {
        "on" | "1" | "true" | "yes" => Ok(true),
        "off" | "0" | "false" | "no" => Ok(false),
        _ => bail!("expected on or off"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolutions() {
        assert_eq!(parse_resolution("4k").unwrap(), (3840, 2160));
        assert_eq!(parse_resolution("1280x720").unwrap(), (1280, 720));
        assert!(parse_resolution("big").is_err());
    }
}
