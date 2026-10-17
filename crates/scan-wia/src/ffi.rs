//! Safe wrappers around single COM calls and `PROPVARIANT` reads. Every `unsafe` block of the
//! backend, except the callback objects in `sinks`, is in this file.
//!
//! The pattern is the same throughout. A COM interface from the `windows` crate owns one reference
//! to a live object, so calling a method on it never dereferences a dangling interface pointer. The
//! methods are `unsafe` only because they take raw pointers; each wrapper here builds those from
//! references to locals that outlive the call, so the arguments are valid by construction. Nothing
//! here keeps a raw pointer past its call.
//!
//! COM must be initialised on the calling thread ([`ComInit`]). A call on a thread that is not
//! initialised fails with an error HRESULT; it is not undefined behaviour, so the wrappers do not
//! need proof of initialisation.
#![allow(unsafe_code)]

use std::marker::PhantomData;
use std::mem::ManuallyDrop;

use windows::Win32::Devices::ImageAcquisition::{
    IEnumWIA_DEV_INFO, IEnumWiaItem2, IWiaDataCallback, IWiaDataTransfer, IWiaDevMgr, IWiaDevMgr2, IWiaItem, IWiaItem2, IWiaPropertyStorage,
    IWiaTransfer, IWiaTransferCallback, WIA_CMD_STOP_FEEDER, WIA_DATA_TRANSFER_INFO, WIA_DEVINFO_ENUM_LOCAL,
};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::StructuredStorage::{
    IEnumSTATPROPSTG, PROPSPEC, PROPSPEC_0, PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0, PRSPEC_PROPID, STATPROPSTG,
};
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Variant::{VARENUM, VT_BSTR, VT_CLSID, VT_I2, VT_I4, VT_INT, VT_LPWSTR, VT_UI2, VT_UI4, VT_UINT, VT_VECTOR};
use windows::core::{BOOL, BSTR, GUID, Interface};

/// A list or vector property longer than this is treated as garbage and read as empty.
const MAX_VECTOR: usize = 64;

/// Bound on the properties one dump walks: a bad enumerator must not run forever.
const MAX_PROPERTIES: u32 = 512;

// ---------------------------------------------------------------------------------------------
// COM apartment
// ---------------------------------------------------------------------------------------------

/// COM initialised on this thread, undone on drop. It is `!Send`: `CoUninitialize` must run on the
/// thread that called `CoInitializeEx`, so the guard cannot move to another thread.
pub struct ComInit {
    uninit: bool,
    _same_thread: PhantomData<*const ()>,
}

impl ComInit {
    /// Enter a single-threaded apartment. A thread that is already in another mode keeps it, and the
    /// guard then undoes nothing.
    pub fn new() -> windows::core::Result<Self> {
        // SAFETY: the reserved argument must be `None`, which it is. The call has no other pointer
        // argument and touches only this thread's apartment state.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            return Ok(Self { uninit: false, _same_thread: PhantomData });
        }
        hr.ok()?;
        Ok(Self { uninit: true, _same_thread: PhantomData })
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.uninit {
            // SAFETY: balances the successful `CoInitializeEx` in `new` (`uninit` is only set then).
            // The guard is `!Send`, so this runs on the thread that initialised COM.
            unsafe { CoUninitialize() }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Object creation and device access
// ---------------------------------------------------------------------------------------------

/// Create a COM object out of process (`WiaDevMgr`, `WiaDevMgr2`) and ask for interface `T`.
pub fn create_instance<T: Interface>(class: &GUID) -> windows::core::Result<T> {
    // SAFETY: `class` is a reference to a live GUID and there is no outer aggregate (`None`). The
    // returned interface owns the reference COM handed back.
    unsafe { CoCreateInstance(class, None, CLSCTX_LOCAL_SERVER) }
}

/// WIA 2.0 `CreateDevice`: open the device with this id.
pub fn create_device(mgr: &IWiaDevMgr2, id: &str) -> windows::core::Result<IWiaItem2> {
    let id = BSTR::from(id);
    // SAFETY: `mgr` is a live interface; `id` is a BSTR that outlives the call.
    unsafe { mgr.CreateDevice(0, &id) }
}

/// WIA 1.0 `CreateDevice`, used to select the document source on drivers that need it.
pub fn create_legacy_device(mgr: &IWiaDevMgr, id: &str) -> windows::core::Result<IWiaItem> {
    let id = BSTR::from(id);
    // SAFETY: `mgr` is a live interface; `id` is a BSTR that outlives the call.
    unsafe { mgr.CreateDevice(&id) }
}

/// The items an enumerator yields, one `Next` call at a time. An error ends the iteration after it
/// is yielded once.
pub struct Drain<T> {
    next: Box<dyn FnMut() -> windows::core::Result<Option<T>>>,
    done: bool,
}

impl<T> Iterator for Drain<T> {
    type Item = windows::core::Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match (self.next)() {
            Ok(Some(item)) => Some(Ok(item)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
    }
}

/// Wrap an enumerator's `Next(1, slot, fetched)`. `fetch` receives a pointer to one empty slot and a
/// pointer to the fetched count, both locals of the closure built here and alive for the call.
fn drain<T: 'static>(mut fetch: impl FnMut(*mut Option<T>, *mut u32) -> windows::core::Result<()> + 'static) -> Drain<T> {
    let next = move || {
        loop {
            let mut fetched = 0u32;
            let mut slot: [Option<T>; 1] = [None];
            fetch(slot.as_mut_ptr(), &mut fetched)?;
            if fetched == 0 {
                return Ok(None);
            }
            if let Some(item) = slot[0].take() {
                return Ok(Some(item));
            }
        }
    };
    Drain { next: Box::new(next), done: false }
}

/// The locally installed devices, as their device-info property storages.
pub fn device_infos(mgr: &IWiaDevMgr2) -> windows::core::Result<Drain<IWiaPropertyStorage>> {
    // SAFETY: `mgr` is a live interface and the flag is a plain integer.
    let list: IEnumWIA_DEV_INFO = unsafe { mgr.EnumDeviceInfo(i32::try_from(WIA_DEVINFO_ENUM_LOCAL).unwrap_or(0)) }?;
    Ok(drain(move |slot, fetched| {
        // SAFETY: `list` is a live enumerator owned by this closure. `slot` points to one writable
        // `Option<IWiaPropertyStorage>` and `fetched` to a `u32`, both locals of `drain` that outlive
        // this call; `Next` writes at most `celt` (1) items.
        unsafe { list.Next(1, slot, fetched) }
    }))
}

/// The child items of a device root.
pub fn child_items(root: &IWiaItem2) -> windows::core::Result<Drain<IWiaItem2>> {
    // SAFETY: `root` is a live interface; `None` asks for children of every category.
    let children: IEnumWiaItem2 = unsafe { root.EnumChildItems(None) }?;
    Ok(drain(move |slot, fetched| {
        // SAFETY: `children` is a live enumerator owned by this closure. `slot` and `fetched` point
        // to locals of `drain` that outlive the call, and `celt` is 1 for a one-item slot.
        unsafe { children.Next(1, slot, fetched) }
    }))
}

// ---------------------------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------------------------

fn spec_for(id: u32) -> PROPSPEC {
    PROPSPEC { ulKind: PRSPEC_PROPID, Anonymous: PROPSPEC_0 { propid: id } }
}

/// Read one property. The returned value clears itself when dropped.
pub fn read_property(props: &IWiaPropertyStorage, id: u32) -> windows::core::Result<PROPVARIANT> {
    let spec = spec_for(id);
    let mut value = PROPVARIANT::default();
    // SAFETY: `props` is a live interface. `spec` and `value` are locals that outlive the call, and
    // the count is 1 for the single spec and single variant.
    unsafe { props.ReadMultiple(1, &spec, &mut value) }?;
    Ok(value)
}

/// What `GetPropertyAttributes` reported for one property.
pub struct Attributes {
    /// `WIA_PROP_*` access and valid-values flags.
    pub flags: u32,
    /// The list, range or flag mask. Empty when the call failed.
    pub valid: PROPVARIANT,
    /// Whether the call succeeded. The flags and values are only meaningful then.
    pub status: windows::core::Result<()>,
}

pub fn property_attributes(props: &IWiaPropertyStorage, id: u32) -> Attributes {
    let spec = spec_for(id);
    let mut flags = 0u32;
    let mut valid = PROPVARIANT::default();
    // SAFETY: `props` is a live interface. `spec`, `flags` and `valid` are locals that outlive the
    // call, and the count is 1 for the single spec, flags slot and variant.
    let status = unsafe { props.GetPropertyAttributes(1, &spec, &mut flags, &mut valid) };
    Attributes { flags, valid, status }
}

/// Write one property. `value` is only read.
pub fn write_property(props: &IWiaPropertyStorage, id: u32, value: &PROPVARIANT) -> windows::core::Result<()> {
    let spec = spec_for(id);
    // SAFETY: `props` is a live interface. `spec` and `value` outlive the call and are only read;
    // the count is 1, and 2 is the first id of the name range (unused for id specs).
    unsafe { props.WriteMultiple(1, &spec, value, 2) }
}

/// Write an `i32` property.
pub fn write_i32_property(props: &IWiaPropertyStorage, id: u32, value: i32) -> windows::core::Result<()> {
    write_property(props, id, &PROPVARIANT::from(value))
}

/// Write a `VT_CLSID` property (an image format).
pub fn write_guid_property(props: &IWiaPropertyStorage, id: u32, guid: GUID) -> windows::core::Result<()> {
    let mut stored = guid;
    // Building a union value is safe. `VT_CLSID` points at our stack GUID, which no variant may
    // free, so the variant is wrapped in `ManuallyDrop` and never cleared.
    let value = ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_CLSID,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 { puuid: &mut stored },
            }),
        },
    });
    write_property(props, id, &value)
}

/// One entry of a property enumeration.
pub struct PropertyEntry {
    pub id: u32,
    pub name: String,
    /// The `VARENUM` type tag.
    pub vt: u16,
}

/// The published properties of a storage, at most `MAX_PROPERTIES` of them.
pub struct PropertyEnum {
    list: IEnumSTATPROPSTG,
    calls: u32,
}

/// Start walking the properties of a storage. `None` when the storage cannot enumerate.
pub fn enumerate_properties(props: &IWiaPropertyStorage) -> Option<PropertyEnum> {
    // SAFETY: `props` is a live interface and `Enum` takes no arguments.
    let list = unsafe { props.Enum() }.ok()?;
    Some(PropertyEnum { list, calls: 0 })
}

impl Iterator for PropertyEnum {
    type Item = PropertyEntry;

    fn next(&mut self) -> Option<PropertyEntry> {
        if self.calls >= MAX_PROPERTIES {
            return None;
        }
        self.calls += 1;
        let mut fetched = 0u32;
        let mut slot = [STATPROPSTG::default()];
        // SAFETY: `list` is a live enumerator. `slot` is a one-element array and `fetched` a local,
        // both outliving the call; the slice length is the item count `Next` may write.
        let hr = unsafe { self.list.Next(&mut slot, Some(&mut fetched)) };
        if fetched == 0 || hr.is_err() {
            return None;
        }
        let entry = &slot[0];
        let name = if entry.lpwstrName.is_null() {
            String::new()
        } else {
            // SAFETY: the enumerator returned a non-null, NUL-terminated wide string for this entry.
            // It is read once and never freed: some class drivers return names that are not
            // CoTaskMem blocks, so freeing could fault. The string is leaked, which only matters
            // for this debug dump.
            unsafe { entry.lpwstrName.to_string() }.unwrap_or_default()
        };
        Some(PropertyEntry { id: entry.propid, name, vt: entry.vt.0 })
    }
}

// ---------------------------------------------------------------------------------------------
// PROPVARIANT reads
//
// A PROPVARIANT is a tagged union. `vt()` is a safe read of the tag; each reader below checks the
// tag first and only then reads the union member that tag names, which is the documented layout.
// Pointers inside the variant (strings, GUIDs, element arrays) belong to the variant: they stay
// valid until it is cleared or dropped, so each reader copies the data out before returning and
// keeps nothing borrowed from the variant.
// ---------------------------------------------------------------------------------------------

/// An integer-typed variant (`VT_I4`, `VT_INT`, `VT_UI4`, `VT_UINT`, `VT_I2`, `VT_UI2`) as an `i64`.
pub fn variant_integer(value: &PROPVARIANT) -> Option<i64> {
    let vt = value.vt();
    // SAFETY: the tag `vt` was read from this variant and selects which member is active; each
    // branch reads exactly the member that tag names.
    unsafe {
        let union = &value.Anonymous.Anonymous.Anonymous;
        if vt == VT_I4 || vt == VT_INT {
            Some(i64::from(union.lVal))
        } else if vt == VT_UI4 || vt == VT_UINT {
            Some(i64::from(union.ulVal))
        } else if vt == VT_I2 {
            Some(i64::from(union.iVal))
        } else if vt == VT_UI2 {
            Some(i64::from(union.uiVal))
        } else {
            None
        }
    }
}

/// A string-typed variant (`VT_BSTR`, `VT_LPWSTR`). `None` for any other type; the inner `Err` is a
/// string that is not valid UTF-16.
pub fn variant_text(value: &PROPVARIANT) -> Option<Result<String, String>> {
    let vt = value.vt();
    if vt == VT_BSTR {
        // SAFETY: the tag says `bstrVal` is active. A BSTR variant always holds a valid BSTR (a null
        // one is the empty string), and it is only read here, while `value` is borrowed.
        let text = unsafe { (*value.Anonymous.Anonymous.Anonymous.bstrVal).to_string() };
        return Some(Ok(text));
    }
    if vt == VT_LPWSTR {
        // SAFETY: the tag says `pwszVal` is active.
        let ptr = unsafe { value.Anonymous.Anonymous.Anonymous.pwszVal };
        if ptr.0.is_null() {
            return Some(Ok(String::new()));
        }
        // SAFETY: non-null, and an `LPWSTR` variant holds a NUL-terminated wide string that lives as
        // long as the variant, which is borrowed for this whole call.
        return Some(unsafe { ptr.to_string() }.map_err(|error| error.to_string()));
    }
    None
}

/// A `VT_CLSID` variant. `None` for any other type or a null pointer.
pub fn variant_guid(value: &PROPVARIANT) -> Option<GUID> {
    if value.vt() != VT_CLSID {
        return None;
    }
    // SAFETY: the tag says `puuid` is active.
    let ptr = unsafe { value.Anonymous.Anonymous.Anonymous.puuid };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null, and a `VT_CLSID` variant points at one GUID that lives as long as the
    // variant, which is borrowed for this call. `GUID` is `Copy`.
    Some(unsafe { *ptr })
}

/// Copy `count` elements out of a counted array. Empty for a null pointer, zero, or too many.
fn copy_elements<T: Copy>(ptr: *const T, count: u32) -> Vec<T> {
    let count = usize::try_from(count).unwrap_or(0);
    if ptr.is_null() || count == 0 || count > MAX_VECTOR {
        return Vec::new();
    }
    // SAFETY: the callers take `ptr` and `count` from one counted-array member (`CAL`, `CACLSID`) of
    // a variant that is still borrowed, so the allocation holds at least `count` initialised `T`s
    // and stays alive for this call. The elements are `Copy` and the slice is copied out at once.
    unsafe { std::slice::from_raw_parts(ptr, count) }.to_vec()
}

/// A `VT_I4 | VT_VECTOR` variant as numbers. Empty for any other type.
pub fn variant_i32_vector(value: &PROPVARIANT) -> Vec<i32> {
    if value.vt() != VARENUM(VT_I4.0 | VT_VECTOR.0) {
        return Vec::new();
    }
    // SAFETY: the tag says the counted-array member `cal` is active.
    let list = unsafe { value.Anonymous.Anonymous.Anonymous.cal };
    copy_elements(list.pElems, list.cElems)
}

/// A `VT_CLSID | VT_VECTOR` variant as GUIDs. Empty for any other type.
pub fn variant_guid_vector(value: &PROPVARIANT) -> Vec<GUID> {
    if value.vt() != VARENUM(VT_CLSID.0 | VT_VECTOR.0) {
        return Vec::new();
    }
    // SAFETY: the tag says the counted-array member `cauuid` is active.
    let list = unsafe { value.Anonymous.Anonymous.Anonymous.cauuid };
    copy_elements(list.pElems, list.cElems)
}

// ---------------------------------------------------------------------------------------------
// Transfers
// ---------------------------------------------------------------------------------------------

/// Run a WIA 2.0 file/stream transfer. WIA calls `callback` on this thread while it runs.
pub fn download(transfer: &IWiaTransfer, callback: &IWiaTransferCallback) -> windows::core::Result<()> {
    // SAFETY: `transfer` is a live interface; `callback` is a live COM object that WIA only uses for
    // the duration of the call (it takes its own reference if it keeps one).
    unsafe { transfer.Download(0, callback) }
}

/// Ask a running transfer to stop.
pub fn cancel_transfer(transfer: &IWiaTransfer) -> windows::core::Result<()> {
    // SAFETY: `transfer` is a live interface; `Cancel` takes no arguments.
    unsafe { transfer.Cancel() }
}

/// Tell the document feeder to stop pulling sheets.
pub fn stop_feeder(device: &IWiaItem2) -> windows::core::Result<()> {
    let mut created = None;
    // SAFETY: `device` is a live interface; the command GUID is a constant and `created` is a local
    // that outlives the call (it stays `None` for this command).
    unsafe { device.DeviceCommand(0, &WIA_CMD_STOP_FEEDER, &mut created) }
}

/// Run a banded transfer (`IWiaDataTransfer::idtGetBandedData`). WIA calls `callback` once per band
/// on this thread.
pub fn banded_transfer(data: &IWiaDataTransfer, buffer_size: u32, callback: &IWiaDataCallback) -> windows::core::Result<()> {
    let mut info = WIA_DATA_TRANSFER_INFO {
        ulSize: u32::try_from(std::mem::size_of::<WIA_DATA_TRANSFER_INFO>()).unwrap_or(0),
        ulBufferSize: buffer_size,
        bDoubleBuffer: BOOL(0),
        ..WIA_DATA_TRANSFER_INFO::default()
    };
    // SAFETY: `data` is a live interface. `info` is a local with its size field set, and it outlives
    // the call; `callback` is a live COM object used only during the call.
    unsafe { data.idtGetBandedData(&mut info, callback) }
}

/// `IWiaTransfer` wrapped so another thread can call [`cancel_transfer`] on it.
///
/// This keeps the cancel path of the first implementation, which handed the same interface pointer
/// to the thread that requests the cancel. See `docs/unsafe-audit.md` (known limits).
pub struct CrossThreadTransfer(pub IWiaTransfer);

impl CrossThreadTransfer {
    /// [`cancel_transfer`] on the wrapped interface.
    pub fn cancel(&self) -> windows::core::Result<()> {
        cancel_transfer(&self.0)
    }
}

// SAFETY: the only operation performed through this wrapper on another thread is
// `IWiaTransfer::Cancel`, which WIA documents as callable while `Download` runs. COM calls on a
// proxy from the wrong apartment fail with an error HRESULT (ignored by the caller) rather than
// corrupting memory, and the wrapper holds its own reference, so the object cannot be freed while
// the wrapper exists. The interface is never otherwise used from the second thread.
unsafe impl Send for CrossThreadTransfer {}
// SAFETY: see the `Send` impl; `&CrossThreadTransfer` only allows `Cancel`, which takes `&self`.
unsafe impl Sync for CrossThreadTransfer {}
