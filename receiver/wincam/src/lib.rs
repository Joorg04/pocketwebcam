//! Pocket Webcam's virtual camera for Windows 11: a Media Foundation media source in a DLL.
//!
//! Windows' Frame Server loads this DLL when an app (Zoom, Teams, Chrome, the Camera app)
//! opens the "Pocket Webcam" camera, and asks it for frames. The frames come from the
//! Pocket Webcam app through shared memory (see `shared.rs`). The app registers the camera
//! once with `MFCreateVirtualCamera`; the installer registers this DLL (`regsvr32`, as
//! administrator, because the Frame Server only looks in HKEY_LOCAL_MACHINE).
//!
//! Structure follows VCamSample by Simon Mourier (github.com/smourier/VCamSample, MIT).

pub mod shared;

#[cfg(windows)]
mod activator;
#[cfg(windows)]
mod attributes;
#[cfg(windows)]
mod feed;
#[cfg(windows)]
mod source;
#[cfg(windows)]
mod stream;

#[cfg(windows)]
pub use exports::*;

/// "Not supported" for camera controls (exposure, focus, ...): we have none.
#[cfg(windows)]
fn not_supported() -> windows::core::Error {
    windows::core::HRESULT::from_win32(windows::Win32::Foundation::ERROR_SET_NOT_FOUND.0).into()
}

#[cfg(windows)]
mod exports {
    use std::ffi::c_void;

    use windows::core::{implement, Interface, Ref, Result, BOOL, GUID, HRESULT, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{
        CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, HMODULE, S_FALSE, S_OK,
    };
    use windows::Win32::Media::MediaFoundation::IMFActivate;
    use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    use windows::Win32::System::Registry::{
        RegDeleteTreeW, RegSetKeyValueW, HKEY_LOCAL_MACHINE, REG_SZ,
    };

    use crate::activator::Activator;
    use crate::shared;

    #[implement(IClassFactory)]
    struct Factory;

    impl IClassFactory_Impl for Factory_Impl {
        fn CreateInstance(&self, outer: Ref<windows_core::IUnknown>, iid: *const GUID, out: *mut *mut c_void) -> Result<()> {
            if out.is_null() || iid.is_null() {
                return Err(E_POINTER.into());
            }
            // SAFETY: checked non-null.
            unsafe { *out = std::ptr::null_mut() };
            if outer.is_some() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let activator: IMFActivate = Activator::new()?.into();
            // SAFETY: checked non-null.
            unsafe { activator.query(iid, out).ok() }
        }

        fn LockServer(&self, _lock: BOOL) -> Result<()> {
            Ok(())
        }
    }

    /// # Safety
    /// Called by COM with valid pointers.
    #[no_mangle]
    pub unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
        if clsid.is_null() || iid.is_null() || out.is_null() {
            return E_POINTER;
        }
        *out = std::ptr::null_mut();
        if *clsid != GUID::from_u128(shared::CLSID) {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(iid, out)
    }

    /// Stays loaded: the Frame Server keeps using the camera for as long as it runs.
    #[no_mangle]
    pub extern "system" fn DllCanUnloadNow() -> HRESULT {
        S_FALSE
    }

    /// `regsvr32 pocketwebcam_camera.dll` (as administrator): registers the class in HKLM.
    #[no_mangle]
    pub extern "system" fn DllRegisterServer() -> HRESULT {
        match register() {
            Ok(()) => S_OK,
            Err(e) => e.code(),
        }
    }

    /// `regsvr32 /u pocketwebcam_camera.dll`.
    #[no_mangle]
    pub extern "system" fn DllUnregisterServer() -> HRESULT {
        let key = HSTRING::from(format!(r"Software\Classes\CLSID\{}", shared::CLSID_STR));
        // SAFETY: plain registry call.
        let status = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, &key) };
        if status.is_ok() {
            S_OK
        } else {
            status.to_hresult()
        }
    }

    fn register() -> Result<()> {
        let path = module_path()?;
        let key = HSTRING::from(format!(r"Software\Classes\CLSID\{}\InprocServer32", shared::CLSID_STR));
        set_string(&key, None, &path)?;
        set_string(&key, Some("ThreadingModel"), "Both")?;
        let name_key = HSTRING::from(format!(r"Software\Classes\CLSID\{}", shared::CLSID_STR));
        set_string(&name_key, None, "Pocket Webcam camera")
    }

    fn set_string(key: &HSTRING, value: Option<&str>, data: &str) -> Result<()> {
        let value = value.map(HSTRING::from);
        let data: Vec<u16> = data.encode_utf16().chain(Some(0)).collect();
        // SAFETY: NUL-terminated strings that outlive the call.
        unsafe {
            RegSetKeyValueW(
                HKEY_LOCAL_MACHINE,
                key,
                value.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            )
            .ok()
        }
    }

    /// Full path of this DLL.
    fn module_path() -> Result<String> {
        let mut module = HMODULE::default();
        let mut buf = [0u16; 1024];
        // SAFETY: looks up the module containing this function; no refcount change.
        unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                PCWSTR(module_path as *const () as *const u16),
                &mut module,
            )?;
            let len = GetModuleFileNameW(Some(module), &mut buf) as usize;
            Ok(String::from_utf16_lossy(&buf[..len]))
        }
    }
}
