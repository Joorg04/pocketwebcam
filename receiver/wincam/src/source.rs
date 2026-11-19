//! The media source Windows' Frame Server talks to: one stream, live, no controls. Follows
//! the structure of VCamSample's MediaSource (github.com/smourier/VCamSample, MIT).

use std::sync::Mutex;

use windows::core::{implement, w, ComObject, IUnknownImpl, Interface, Ref, Result, BOOL, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Media::KernelStreaming::{
    IKsControl, IKsControl_Impl, KSCAMERAPROFILE_HighFrameRate, KSCAMERAPROFILE_Legacy, KSIDENTIFIER,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

use crate::attributes::delegate_attributes;
use crate::not_supported;
use crate::stream::Stream;

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl, IMFSampleAllocatorControl, IMFAttributes)]
pub struct Source {
    attrs: IMFAttributes,
    /// The single stream (`Stream` object), kept as its COM object.
    stream: ComObject<Stream>,
    state: Mutex<State>,
}

struct State {
    queue: Option<IMFMediaEventQueue>,
    descriptor: Option<IMFPresentationDescriptor>,
}

delegate_attributes!(Source_Impl);

impl Source {
    /// Creates the source, copying the activator's attributes.
    pub fn create(activator_attrs: &IMFAttributes) -> Result<ComObject<Source>> {
        // SAFETY: plain Media Foundation calls on objects we own.
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 8)?;
            let attrs = attrs.ok_or(windows::core::Error::from(E_POINTER))?;
            activator_attrs.CopyAllItems(&attrs)?;

            // Sensor profiles: a "legacy" one (up to 30 fps) and a high frame rate one.
            let profiles = MFCreateSensorProfileCollection()?;
            let legacy = MFCreateSensorProfile(&KSCAMERAPROFILE_Legacy, 0, PCWSTR::null())?;
            legacy.AddProfileFilter(0, w!("((RES==;FRT<=30,1;SUT==))"))?;
            profiles.AddProfile(&legacy)?;
            let fast = MFCreateSensorProfile(&KSCAMERAPROFILE_HighFrameRate, 0, PCWSTR::null())?;
            fast.AddProfileFilter(0, w!("((RES==;FRT>=60,1;SUT==))"))?;
            profiles.AddProfile(&fast)?;
            attrs.SetUnknown(&MF_DEVICEMFT_SENSORPROFILE_COLLECTION, &profiles)?;

            let stream = ComObject::new(Stream::new(0)?);
            let descriptor = MFCreatePresentationDescriptor(Some(&[Some(stream.descriptor()?)]))?;
            let source = ComObject::new(Source {
                attrs,
                stream: stream.clone(),
                state: Mutex::new(State { queue: Some(MFCreateEventQueue()?), descriptor: Some(descriptor) }),
            });
            stream.set_source(source.to_interface::<IMFMediaSourceEx>().cast()?);
            Ok(source)
        }
    }

    fn queue(&self) -> Result<IMFMediaEventQueue> {
        self.state.lock().unwrap().queue.clone().ok_or(MF_E_SHUTDOWN.into())
    }

    fn descriptor(&self) -> Result<IMFPresentationDescriptor> {
        self.state.lock().unwrap().descriptor.clone().ok_or(MF_E_SHUTDOWN.into())
    }
}

/// `MFGetSystemTime()` as a PROPVARIANT, for the started/stopped events.
fn now() -> PROPVARIANT {
    // SAFETY: no preconditions.
    PROPVARIANT::from(unsafe { MFGetSystemTime() })
}

impl IMFMediaEventGenerator_Impl for Source_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        // Never hold the lock here: a blocking GetEvent would stall everything else.
        let queue = self.queue()?;
        unsafe { queue.GetEvent(flags.0) }
    }
    fn BeginGetEvent(&self, callback: Ref<IMFAsyncCallback>, state: Ref<windows_core::IUnknown>) -> Result<()> {
        let queue = self.queue()?;
        unsafe { queue.BeginGetEvent(callback.as_ref(), state.as_ref()) }
    }
    fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        let queue = self.queue()?;
        unsafe { queue.EndGetEvent(result.as_ref()) }
    }
    fn QueueEvent(&self, met: u32, extended: *const GUID, status: windows_core::HRESULT, value: *const PROPVARIANT) -> Result<()> {
        let queue = self.queue()?;
        unsafe { queue.QueueEventParamVar(met, extended, status, value) }
    }
}

impl IMFMediaSource_Impl for Source_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }

    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        unsafe { self.descriptor()?.Clone() }
    }

    fn Start(&self, requested: Ref<IMFPresentationDescriptor>, time_format: *const GUID, _start: *const PROPVARIANT) -> Result<()> {
        let requested = requested.ok()?;
        // SAFETY: `time_format` is null or points to a GUID, per the interface contract.
        if !time_format.is_null() && unsafe { *time_format } != GUID::zeroed() {
            return Err(windows::Win32::Foundation::E_INVALIDARG.into());
        }
        let (queue, ours) = (self.queue()?, self.descriptor()?);
        // SAFETY: plain Media Foundation calls; one stream, index 0.
        unsafe {
            let (mut selected, mut desc) = (BOOL::default(), None);
            requested.GetStreamDescriptorByIndex(0, &mut selected, &mut desc)?;
            let desc = desc.ok_or(windows::core::Error::from(E_POINTER))?;
            let running = self.stream.stream_state() != MF_STREAM_STATE_STOPPED;
            if selected.as_bool() && !running {
                ours.SelectStream(0)?;
                queue.QueueEventParamUnk(MENewStream.0 as u32, &GUID::zeroed(), Default::default(), &self.stream.to_interface::<IMFMediaStream2>())?;
                let media_type = desc.GetMediaTypeHandler()?.GetCurrentMediaType()?;
                self.stream.start(Some(media_type))?;
            } else if !selected.as_bool() && running {
                ours.DeselectStream(0)?;
                self.stream.stop()?;
            }
            queue.QueueEventParamVar(MESourceStarted.0 as u32, &GUID::zeroed(), Default::default(), &now())?;
        }
        Ok(())
    }

    fn Stop(&self) -> Result<()> {
        let (queue, ours) = (self.queue()?, self.descriptor()?);
        // SAFETY: plain Media Foundation calls.
        unsafe {
            if self.stream.stream_state() != MF_STREAM_STATE_STOPPED {
                self.stream.stop()?;
            }
            ours.DeselectStream(0)?;
            queue.QueueEventParamVar(MESourceStopped.0 as u32, &GUID::zeroed(), Default::default(), &now())?;
        }
        Ok(())
    }

    fn Pause(&self) -> Result<()> {
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }

    fn Shutdown(&self) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        let queue = s.queue.take().ok_or(windows::core::Error::from(MF_E_SHUTDOWN))?;
        // SAFETY: shutting down our own queue.
        let _ = unsafe { queue.Shutdown() };
        s.descriptor = None;
        drop(s);
        self.stream.shutdown();
        Ok(())
    }
}

impl IMFMediaSourceEx_Impl for Source_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        Ok(self.to_interface())
    }

    fn GetStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        if id != 0 {
            return Err(windows::Win32::Foundation::E_INVALIDARG.into());
        }
        Ok(self.stream.to_interface())
    }

    /// We fill samples on the CPU, so the GPU device manager isn't needed.
    fn SetD3DManager(&self, _manager: Ref<windows_core::IUnknown>) -> Result<()> {
        Ok(())
    }
}

impl IMFGetService_Impl for Source_Impl {
    fn GetService(&self, _service: *const GUID, _iid: *const GUID, _out: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

impl IMFSampleAllocatorControl_Impl for Source_Impl {
    fn SetDefaultAllocator(&self, stream_id: u32, allocator: Ref<windows_core::IUnknown>) -> Result<()> {
        if stream_id != 0 {
            return Err(windows::Win32::Foundation::E_INVALIDARG.into());
        }
        self.stream.set_allocator(allocator.ok()?.cast()?);
        Ok(())
    }

    fn GetAllocatorUsage(&self, stream_id: u32, input_id: *mut u32, usage: *mut MFSampleAllocatorUsage) -> Result<()> {
        if input_id.is_null() || usage.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: both pointers checked above.
        unsafe {
            *input_id = stream_id;
            *usage = MFSampleAllocatorUsage_UsesProvidedAllocator;
        }
        Ok(())
    }
}

impl IKsControl_Impl for Source_Impl {
    fn KsProperty(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_supported())
    }
    fn KsMethod(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_supported())
    }
    fn KsEvent(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_supported())
    }
}
