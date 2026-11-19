//! The object Windows creates from our class: it describes the camera and hands out the
//! media source. Follows VCamSample's Activator (github.com/smourier/VCamSample, MIT).

use std::sync::Mutex;

use windows::core::{implement, ComObject, Interface, Ref, Result, BOOL, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

use crate::attributes::delegate_attributes;
use crate::shared;
use crate::source::Source;

#[implement(IMFActivate)]
pub struct Activator {
    attrs: IMFAttributes,
    source: Mutex<Option<ComObject<Source>>>,
}

delegate_attributes!(Activator_Impl);

impl Activator {
    pub fn new() -> Result<Activator> {
        // SAFETY: plain Media Foundation calls on objects we own.
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 4)?;
            let attrs = attrs.ok_or(windows::core::Error::from(E_POINTER))?;
            attrs.SetUINT32(&MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES, 1)?;
            attrs.SetGUID(&MFT_TRANSFORM_CLSID_Attribute, &GUID::from_u128(shared::CLSID))?;
            Ok(Activator { attrs, source: Mutex::new(None) })
        }
    }
}

impl IMFActivate_Impl for Activator_Impl {
    fn ActivateObject(&self, iid: *const GUID, out: *mut *mut core::ffi::c_void) -> Result<()> {
        if out.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        let mut source = self.source.lock().unwrap();
        if source.is_none() {
            *source = Some(Source::create(&self.attrs)?);
        }
        let unknown: windows_core::IUnknown = source.as_ref().map(|s| s.to_interface()).ok_or(E_POINTER)?;
        // SAFETY: `iid` and `out` checked non-null above.
        unsafe { unknown.query(iid, out).ok() }
    }

    fn ShutdownObject(&self) -> Result<()> {
        Ok(())
    }

    fn DetachObject(&self) -> Result<()> {
        *self.source.lock().unwrap() = None;
        Ok(())
    }
}
