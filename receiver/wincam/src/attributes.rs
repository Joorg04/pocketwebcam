//! The activator, the source and the stream each act as an attribute store (Windows
//! reads and writes settings on them). Each keeps a real store from `MFCreateAttributes`
//! in a field called `attrs`, and this macro forwards every `IMFAttributes` call to it.

macro_rules! delegate_attributes {
    ($impl_ty:ty) => {
        impl windows::Win32::Media::MediaFoundation::IMFAttributes_Impl for $impl_ty {
            fn GetItem(&self, key: *const GUID, value: *mut PROPVARIANT) -> Result<()> {
                unsafe { self.attrs.GetItem(key, Some(value)) }
            }
            fn GetItemType(&self, key: *const GUID) -> Result<MF_ATTRIBUTE_TYPE> {
                unsafe { self.attrs.GetItemType(key) }
            }
            fn CompareItem(&self, key: *const GUID, value: *const PROPVARIANT) -> Result<BOOL> {
                unsafe { self.attrs.CompareItem(key, value) }
            }
            fn Compare(&self, theirs: Ref<IMFAttributes>, match_type: MF_ATTRIBUTES_MATCH_TYPE) -> Result<BOOL> {
                unsafe { self.attrs.Compare(theirs.as_ref(), match_type) }
            }
            fn GetUINT32(&self, key: *const GUID) -> Result<u32> {
                unsafe { self.attrs.GetUINT32(key) }
            }
            fn GetUINT64(&self, key: *const GUID) -> Result<u64> {
                unsafe { self.attrs.GetUINT64(key) }
            }
            fn GetDouble(&self, key: *const GUID) -> Result<f64> {
                unsafe { self.attrs.GetDouble(key) }
            }
            fn GetGUID(&self, key: *const GUID) -> Result<GUID> {
                unsafe { self.attrs.GetGUID(key) }
            }
            fn GetStringLength(&self, key: *const GUID) -> Result<u32> {
                unsafe { self.attrs.GetStringLength(key) }
            }
            fn GetString(&self, key: *const GUID, buf: PWSTR, size: u32, len: *mut u32) -> Result<()> {
                unsafe {
                    // A size query passes a null buffer; a slice may not be made from null.
                    let buf: &mut [u16] = if buf.0.is_null() { &mut [] } else { std::slice::from_raw_parts_mut(buf.0, size as usize) };
                    self.attrs.GetString(key, buf, (!len.is_null()).then_some(len))
                }
            }
            fn GetAllocatedString(&self, key: *const GUID, value: *mut PWSTR, len: *mut u32) -> Result<()> {
                unsafe { self.attrs.GetAllocatedString(key, value, len) }
            }
            fn GetBlobSize(&self, key: *const GUID) -> Result<u32> {
                unsafe { self.attrs.GetBlobSize(key) }
            }
            fn GetBlob(&self, key: *const GUID, buf: *mut u8, size: u32, len: *mut u32) -> Result<()> {
                unsafe {
                    let buf: &mut [u8] = if buf.is_null() { &mut [] } else { std::slice::from_raw_parts_mut(buf, size as usize) };
                    self.attrs.GetBlob(key, buf, (!len.is_null()).then_some(len))
                }
            }
            fn GetAllocatedBlob(&self, key: *const GUID, buf: *mut *mut u8, size: *mut u32) -> Result<()> {
                unsafe { self.attrs.GetAllocatedBlob(key, buf, size) }
            }
            fn GetUnknown(&self, key: *const GUID, iid: *const GUID, out: *mut *mut core::ffi::c_void) -> Result<()> {
                // The safe wrapper is generic over the result type; call the store's vtable.
                unsafe {
                    (windows_core::Interface::vtable(&self.attrs).GetUnknown)(
                        windows_core::Interface::as_raw(&self.attrs),
                        key,
                        iid,
                        out,
                    )
                    .ok()
                }
            }
            fn SetItem(&self, key: *const GUID, value: *const PROPVARIANT) -> Result<()> {
                unsafe { self.attrs.SetItem(key, value) }
            }
            fn DeleteItem(&self, key: *const GUID) -> Result<()> {
                unsafe { self.attrs.DeleteItem(key) }
            }
            fn DeleteAllItems(&self) -> Result<()> {
                unsafe { self.attrs.DeleteAllItems() }
            }
            fn SetUINT32(&self, key: *const GUID, value: u32) -> Result<()> {
                unsafe { self.attrs.SetUINT32(key, value) }
            }
            fn SetUINT64(&self, key: *const GUID, value: u64) -> Result<()> {
                unsafe { self.attrs.SetUINT64(key, value) }
            }
            fn SetDouble(&self, key: *const GUID, value: f64) -> Result<()> {
                unsafe { self.attrs.SetDouble(key, value) }
            }
            fn SetGUID(&self, key: *const GUID, value: *const GUID) -> Result<()> {
                unsafe { self.attrs.SetGUID(key, value) }
            }
            fn SetString(&self, key: *const GUID, value: &PCWSTR) -> Result<()> {
                unsafe { self.attrs.SetString(key, *value) }
            }
            fn SetBlob(&self, key: *const GUID, buf: *const u8, size: u32) -> Result<()> {
                let buf: &[u8] = if buf.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(buf, size as usize) } };
                unsafe { self.attrs.SetBlob(key, buf) }
            }
            fn SetUnknown(&self, key: *const GUID, value: Ref<windows_core::IUnknown>) -> Result<()> {
                unsafe { self.attrs.SetUnknown(key, value.as_ref()) }
            }
            fn LockStore(&self) -> Result<()> {
                unsafe { self.attrs.LockStore() }
            }
            fn UnlockStore(&self) -> Result<()> {
                unsafe { self.attrs.UnlockStore() }
            }
            fn GetCount(&self) -> Result<u32> {
                unsafe { self.attrs.GetCount() }
            }
            fn GetItemByIndex(&self, index: u32, key: *mut GUID, value: *mut PROPVARIANT) -> Result<()> {
                unsafe { self.attrs.GetItemByIndex(index, key, Some(value)) }
            }
            fn CopyAllItems(&self, dest: Ref<IMFAttributes>) -> Result<()> {
                unsafe { self.attrs.CopyAllItems(dest.as_ref()) }
            }
        }
    };
}

pub(crate) use delegate_attributes;
