//! The OLE server for embedded-object activation (genoffice#1985 stage 3).
//! `--ole-server` registers the GenOffice class object with COM
//! (CLSCTX_LOCAL_SERVER) and pumps messages; when a container (Word, a CAD's
//! paste dialog) double-clicks a pasted GenOffice object, ole32 launches this
//! helper through the LocalServer32 registry value, the class factory hands
//! out the object, and the container drives it through the classic OLE
//! interfaces:
//!
//! - `IPersistStorage::Load` receives the object's IStorage (marshaled from
//!   the container); we keep it and read the presentation extents.
//! - `IOleObject::DoVerb` (PRIMARY/SHOW/OPEN) extracts the workbook from the
//!   `\1Ole10Native` stream, writes it to a temp file and launches GenOffice
//!   with it — the "open for editing" half of the issue, minus in-place UI
//!   activation (the container's menus are not taken over; GenOffice opens
//!   its own window, and edits do not stream back into the container).
//! - `IDataObject::GetData` re-serves CF_ENHMETAFILE from `\3OlePres000` so
//!   containers that lost their presentation cache can still draw the object.
//!
//! windows-sys declares interfaces as opaque pointers, so the vtables below
//! are hand-written. Every method order and signature here was checked
//! against the `*_Vtbl` definitions in microsoft/windows-rs (objidl, oleidl,
//! objidlbase, unknwnbase), which mirror the SDK headers.
//!
//! Calls arrive serialized through the STA message pump because the class
//! object is registered on this single apartment thread, so the plain
//! (non-atomic) object state below is never touched concurrently.

// The whole file is FFI vtable glue: every function body is raw-pointer and
// foreign-call work, so wrapping each statement in its own `unsafe` block
// would add noise without marking any boundary.
#![allow(unsafe_op_in_unsafe_fn)]

use crate::embed::{native_workbook_bytes, ole10_native_name, olepres_emf_bytes, olepres_name};
use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::Foundation::{
    CLASS_E_NOAGGREGATION, DATA_S_SAMEFORMATETC, DV_E_DVASPECT, DV_E_FORMATETC, DV_E_TYMED, E_FAIL,
    E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, GlobalFree, HGLOBAL, HWND,
    OLE_E_ADVISENOTSUPPORTED, OLE_E_NOCONNECTION, RECT, RPC_E_CHANGED_MODE, S_FALSE, S_OK, SIZE,
};
use windows_sys::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoInitializeEx, CoRegisterClassObject,
    CoRevokeClassObject, CoTaskMemAlloc, DVASPECT_CONTENT, FORMATETC, REGCLS_MULTIPLEUSE,
    STGM_READ, STGM_SHARE_EXCLUSIVE, STGMEDIUM, TYMED_HGLOBAL,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::{OLEIVERB_OPEN, OLEIVERB_PRIMARY, OLEIVERB_SHOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG};
use windows_sys::core::{GUID, HRESULT, PCWSTR, PWSTR};

type This = *mut core::ffi::c_void;

/// The embedded object's class, in the Windows GUID layout. Must stay the
/// same value as embed::GENOFFICE_SHEET_CLSID (a test in embed.rs pins the
/// two together).
pub const CLSID_GENOFFICE_SHEET: GUID = GUID::from_u128(0x8C10_B647_B13C_4511_8B4F_2E31_E5A9_D7C6);

/// The well-known OLE interface IIDs (windows-sys only exports IID_IUnknown).
const IID_IUNKNOWN: GUID = GUID::from_u128(0x0000_0000_0000_0000_c000_0000_0000_0046);
const IID_ICLASS_FACTORY: GUID = GUID::from_u128(0x0000_0001_0000_0000_c000_0000_0000_0046);
const IID_IPERSIST: GUID = GUID::from_u128(0x0000_010c_0000_0000_c000_0000_0000_0046);
const IID_IPERSIST_STORAGE: GUID = GUID::from_u128(0x0000_010a_0000_0000_c000_0000_0000_0046);
const IID_IDATA_OBJECT: GUID = GUID::from_u128(0x0000_010e_0000_0000_c000_0000_0000_0046);
const IID_IOLE_OBJECT: GUID = GUID::from_u128(0x0000_0112_0000_0000_c000_0000_0000_0046);

const CF_ENHMETAFILE: u16 = 14;

/// Byte offsets of the interface slots inside OleObject (see the struct).
const OO_SLOT: usize = core::mem::size_of::<usize>();
const DO_SLOT: usize = 2 * core::mem::size_of::<usize>();

/// windows-sys' GUID carries no PartialEq.
fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

// ---------------------------------------------------------------- vtables

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(This, *const GUID, *mut This) -> HRESULT,
    add_ref: unsafe extern "system" fn(This) -> u32,
    release: unsafe extern "system" fn(This) -> u32,
}

/// IPersistStorage_Vtbl: IPersist_Vtbl (IUnknown + GetClassID) then IsDirty,
/// InitNew, Load, Save, SaveCompleted, HandsOffStorage.
#[repr(C)]
struct IPersistStorageVtbl {
    unknown: IUnknownVtbl,
    get_class_id: unsafe extern "system" fn(This, *mut GUID) -> HRESULT,
    is_dirty: unsafe extern "system" fn(This) -> HRESULT,
    init_new: unsafe extern "system" fn(This, This) -> HRESULT,
    load: unsafe extern "system" fn(This, This) -> HRESULT,
    save: unsafe extern "system" fn(This, This, i32) -> HRESULT,
    save_completed: unsafe extern "system" fn(This, This) -> HRESULT,
    hands_off_storage: unsafe extern "system" fn(This) -> HRESULT,
}

/// IOleObject_Vtbl: IUnknown then the 21 methods in oleidl order.
#[repr(C)]
struct IOleObjectVtbl {
    unknown: IUnknownVtbl,
    set_client_site: unsafe extern "system" fn(This, This) -> HRESULT,
    get_client_site: unsafe extern "system" fn(This, *mut This) -> HRESULT,
    set_host_names: unsafe extern "system" fn(This, PCWSTR, PCWSTR) -> HRESULT,
    close: unsafe extern "system" fn(This, u32) -> HRESULT,
    set_moniker: unsafe extern "system" fn(This, u32, This) -> HRESULT,
    get_moniker: unsafe extern "system" fn(This, u32, u32, *mut This) -> HRESULT,
    init_from_data: unsafe extern "system" fn(This, This, i32, u32) -> HRESULT,
    get_clipboard_data: unsafe extern "system" fn(This, u32, *mut This) -> HRESULT,
    /// (iVerb, lpmsg, pActiveSite, lindex, hwndParent, lprcPosRect) — the
    /// rectangle is a pointer (LPCRECT), not a by-value RECT.
    do_verb: unsafe extern "system" fn(This, i32, This, This, i32, HWND, *const RECT) -> HRESULT,
    enum_verbs: unsafe extern "system" fn(This, *mut This) -> HRESULT,
    update: unsafe extern "system" fn(This) -> HRESULT,
    is_up_to_date: unsafe extern "system" fn(This) -> HRESULT,
    get_user_class_id: unsafe extern "system" fn(This, *mut GUID) -> HRESULT,
    get_user_type: unsafe extern "system" fn(This, u32, *mut PWSTR) -> HRESULT,
    set_extent: unsafe extern "system" fn(This, u32, *const SIZE) -> HRESULT,
    get_extent: unsafe extern "system" fn(This, u32, *mut SIZE) -> HRESULT,
    advise: unsafe extern "system" fn(This, This, *mut u32) -> HRESULT,
    unadvise: unsafe extern "system" fn(This, u32) -> HRESULT,
    enum_advise: unsafe extern "system" fn(This, *mut This) -> HRESULT,
    get_misc_status: unsafe extern "system" fn(This, u32, *mut u32) -> HRESULT,
    set_color_scheme: unsafe extern "system" fn(This, *const core::ffi::c_void) -> HRESULT,
}

/// IDataObject_Vtbl: IUnknown then the 9 methods in objidl order.
#[repr(C)]
struct IDataObjectVtbl {
    unknown: IUnknownVtbl,
    get_data: unsafe extern "system" fn(This, *const FORMATETC, *mut STGMEDIUM) -> HRESULT,
    get_data_here: unsafe extern "system" fn(This, *const FORMATETC, *mut STGMEDIUM) -> HRESULT,
    query_get_data: unsafe extern "system" fn(This, *const FORMATETC) -> HRESULT,
    get_canonical_format_etc:
        unsafe extern "system" fn(This, *const FORMATETC, *mut FORMATETC) -> HRESULT,
    set_data: unsafe extern "system" fn(This, *const FORMATETC, *const STGMEDIUM, i32) -> HRESULT,
    enum_format_etc: unsafe extern "system" fn(This, u32, *mut This) -> HRESULT,
    d_advise: unsafe extern "system" fn(This, *const FORMATETC, u32, This, *mut u32) -> HRESULT,
    d_unadvise: unsafe extern "system" fn(This, u32) -> HRESULT,
    enum_d_advise: unsafe extern "system" fn(This, *mut This) -> HRESULT,
}

/// IClassFactory_Vtbl: IUnknown, CreateInstance, LockServer.
#[repr(C)]
struct IClassFactoryVtbl {
    unknown: IUnknownVtbl,
    create_instance: unsafe extern "system" fn(This, This, *const GUID, *mut This) -> HRESULT,
    lock_server: unsafe extern "system" fn(This, i32) -> HRESULT,
}

// ------------------------------------------------------------- the object

/// One COM object, three interfaces. The first three fields are the
/// interface slots a container's pointer lands on (each holds the address of
/// that interface's vtable); everything after is the shared state every
/// method recovers by subtracting its slot offset.
#[repr(C)]
struct OleObject {
    persist_storage_slot: *const IPersistStorageVtbl,
    ole_object_slot: *const IOleObjectVtbl,
    data_object_slot: *const IDataObjectVtbl,
    ref_count: AtomicU32,
    /// AddRef'd IStorage the container marshaled to IPersistStorage::Load.
    storage: This,
    /// IOleClientSite from SetClientSite (AddRef'd), or null.
    client_site: This,
    width_himetric: u32,
    height_himetric: u32,
    /// DoVerb launched GenOffice once; later verbs are no-ops so a second
    /// double-click does not stack windows.
    opened: bool,
}

/// Generates one interface's IUnknown trio: each recovers the object from
/// its own slot offset, so a QueryInterface made through the IOleObject or
/// IDataObject pointer still finds the right base.
macro_rules! unknown_thunks {
    ($query:ident, $add:ident, $release:ident, $slot:expr) => {
        unsafe extern "system" fn $query(this: This, riid: *const GUID, out: *mut This) -> HRESULT {
            object_query_interface(base_of(this, $slot), riid, out)
        }
        unsafe extern "system" fn $add(this: This) -> u32 {
            object_add_ref(base_of(this, $slot))
        }
        unsafe extern "system" fn $release(this: This) -> u32 {
            object_release(base_of(this, $slot))
        }
    };
}
unknown_thunks!(ps_query_interface, ps_add_ref, ps_release, 0);
unknown_thunks!(oo_query_interface, oo_add_ref, oo_release, OO_SLOT);
unknown_thunks!(do_query_interface, do_add_ref, do_release, DO_SLOT);

static PERSIST_STORAGE_VTBL: IPersistStorageVtbl = IPersistStorageVtbl {
    unknown: IUnknownVtbl {
        query_interface: ps_query_interface,
        add_ref: ps_add_ref,
        release: ps_release,
    },
    get_class_id: ps_get_class_id,
    is_dirty: ps_is_dirty,
    init_new: ps_init_new,
    load: ps_load,
    save: ps_save,
    save_completed: ps_save_completed,
    hands_off_storage: ps_hands_off_storage,
};

static OLE_OBJECT_VTBL: IOleObjectVtbl = IOleObjectVtbl {
    unknown: IUnknownVtbl {
        query_interface: oo_query_interface,
        add_ref: oo_add_ref,
        release: oo_release,
    },
    set_client_site: oo_set_client_site,
    get_client_site: oo_get_client_site,
    set_host_names: oo_set_host_names,
    close: oo_close,
    set_moniker: oo_set_moniker,
    get_moniker: oo_get_moniker,
    init_from_data: oo_init_from_data,
    get_clipboard_data: oo_get_clipboard_data,
    do_verb: oo_do_verb,
    enum_verbs: oo_enum_verbs,
    update: oo_update,
    is_up_to_date: oo_is_up_to_date,
    get_user_class_id: oo_get_user_class_id,
    get_user_type: oo_get_user_type,
    set_extent: oo_set_extent,
    get_extent: oo_get_extent,
    advise: oo_advise,
    unadvise: oo_unadvise,
    enum_advise: oo_enum_advise,
    get_misc_status: oo_get_misc_status,
    set_color_scheme: oo_set_color_scheme,
};

static DATA_OBJECT_VTBL: IDataObjectVtbl = IDataObjectVtbl {
    unknown: IUnknownVtbl {
        query_interface: do_query_interface,
        add_ref: do_add_ref,
        release: do_release,
    },
    get_data: do_get_data,
    get_data_here: do_get_data_here,
    query_get_data: do_query_get_data,
    get_canonical_format_etc: do_get_canonical_format_etc,
    set_data: do_set_data,
    enum_format_etc: do_enum_format_etc,
    d_advise: do_d_advise,
    d_unadvise: do_d_unadvise,
    enum_d_advise: do_enum_d_advise,
};

static CLASS_FACTORY_VTBL: IClassFactoryVtbl = IClassFactoryVtbl {
    unknown: IUnknownVtbl {
        query_interface: factory_query_interface,
        add_ref: factory_add_ref,
        release: factory_release,
    },
    create_instance: factory_create_instance,
    lock_server: factory_lock_server,
};

/// The class factory singleton handed to CoRegisterClassObject. It lives for
/// the whole process, so its reference count is bookkeeping only.
#[repr(C)]
struct ClassFactory {
    slot: *const IClassFactoryVtbl,
    ref_count: AtomicU32,
}
// The only field shared across threads is the atomic counter; the vtable
// pointer is immutable.
unsafe impl Sync for ClassFactory {}
static CLASS_FACTORY: ClassFactory = ClassFactory {
    slot: &CLASS_FACTORY_VTBL,
    ref_count: AtomicU32::new(1),
};

impl OleObject {
    fn boxed() -> *mut OleObject {
        Box::into_raw(Box::new(OleObject {
            persist_storage_slot: &PERSIST_STORAGE_VTBL,
            ole_object_slot: &OLE_OBJECT_VTBL,
            data_object_slot: &DATA_OBJECT_VTBL,
            ref_count: AtomicU32::new(1),
            storage: core::ptr::null_mut(),
            client_site: core::ptr::null_mut(),
            width_himetric: 0,
            height_himetric: 0,
            opened: false,
        }))
    }
}

/// Recovers the object from an interface pointer that sits `slot` bytes in.
unsafe fn base_of(this: This, slot: usize) -> *mut OleObject {
    (this as *mut u8).sub(slot) as *mut OleObject
}

/// The interface pointer for the slot `slot` bytes into the object.
unsafe fn slot_ptr(base: *mut OleObject, slot: usize) -> This {
    (base as *mut u8).add(slot) as This
}

/// Reads entry `index` of a foreign COM object's vtable.
unsafe fn foreign_slot(object: This, index: usize) -> usize {
    let vtbl = *(object as *const *const usize);
    *vtbl.add(index)
}

/// IUnknown::AddRef (vtable slot 1) on a foreign interface pointer.
unsafe fn foreign_add_ref(object: This) {
    if !object.is_null() {
        let add_ref: unsafe extern "system" fn(This) -> u32 =
            core::mem::transmute(foreign_slot(object, 1));
        add_ref(object);
    }
}

/// IUnknown::Release (vtable slot 2) on a foreign interface pointer.
unsafe fn foreign_release(object: This) {
    if !object.is_null() {
        let release: unsafe extern "system" fn(This) -> u32 =
            core::mem::transmute(foreign_slot(object, 2));
        release(object);
    }
}

// --------------------------------------------------- shared IUnknown core

unsafe fn object_query_interface(
    base: *mut OleObject,
    riid: *const GUID,
    out: *mut This,
) -> HRESULT {
    if riid.is_null() || out.is_null() {
        return E_POINTER;
    }
    let iid = &*riid;
    // IUnknown answers with one canonical pointer (COM's identity rule), the
    // IPersistStorage slot at offset 0; IPersist shares that vtable prefix.
    let found = if guid_eq(iid, &IID_IUNKNOWN)
        || guid_eq(iid, &IID_IPERSIST_STORAGE)
        || guid_eq(iid, &IID_IPERSIST)
    {
        slot_ptr(base, 0)
    } else if guid_eq(iid, &IID_IOLE_OBJECT) {
        slot_ptr(base, OO_SLOT)
    } else if guid_eq(iid, &IID_IDATA_OBJECT) {
        slot_ptr(base, DO_SLOT)
    } else {
        *out = core::ptr::null_mut();
        return E_NOINTERFACE;
    };
    (*base).ref_count.fetch_add(1, Ordering::Relaxed);
    *out = found;
    S_OK
}

unsafe fn object_add_ref(base: *mut OleObject) -> u32 {
    (*base).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe fn object_release(base: *mut OleObject) -> u32 {
    let left = (*base).ref_count.fetch_sub(1, Ordering::Release) - 1;
    if left == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        foreign_release((*base).storage);
        foreign_release((*base).client_site);
        drop(Box::from_raw(base));
    }
    left
}

// -------------------------------------------------- IPersistStorage impls

unsafe extern "system" fn ps_get_class_id(_this: This, out: *mut GUID) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    *out = CLSID_GENOFFICE_SHEET;
    S_OK
}

/// We never stream saves back into the container's storage (the workbook is
/// edited as a file by GenOffice), so the object is never dirty.
unsafe extern "system" fn ps_is_dirty(_this: This) -> HRESULT {
    S_FALSE
}

unsafe extern "system" fn ps_init_new(_this: This, _storage: This) -> HRESULT {
    S_OK
}

/// The container hands over the object's storage (the copy's Embed Source).
/// Keep it and read the presentation extents; the workbook itself is only
/// extracted on DoVerb, so pasting never launches anything.
unsafe extern "system" fn ps_load(this: This, storage: This) -> HRESULT {
    if storage.is_null() {
        return E_POINTER;
    }
    let base = base_of(this, 0);
    foreign_add_ref(storage);
    foreign_release((*base).storage);
    (*base).storage = storage;
    (*base).width_himetric = 0;
    (*base).height_himetric = 0;
    if let Some(pres) = read_stream(storage, &olepres_name()) {
        if pres.len() >= 36 {
            (*base).width_himetric = u32::from_le_bytes([pres[28], pres[29], pres[30], pres[31]]);
            (*base).height_himetric = u32::from_le_bytes([pres[32], pres[33], pres[34], pres[35]]);
        }
    }
    S_OK
}

/// A no-op on purpose: GenOffice edits the extracted workbook file, and the
/// container's copy stays the paste-time snapshot (stated in the PR; a live
/// two-way embed belongs to full in-place activation).
unsafe extern "system" fn ps_save(_this: This, _storage: This, _same_as_load: i32) -> HRESULT {
    S_OK
}

unsafe extern "system" fn ps_save_completed(this: This, storage: This) -> HRESULT {
    if !storage.is_null() {
        let base = base_of(this, 0);
        foreign_add_ref(storage);
        foreign_release((*base).storage);
        (*base).storage = storage;
    }
    S_OK
}

unsafe extern "system" fn ps_hands_off_storage(this: This) -> HRESULT {
    let base = base_of(this, 0);
    foreign_release((*base).storage);
    (*base).storage = core::ptr::null_mut();
    S_OK
}

// ----------------------------------------------------- IOleObject impls

unsafe extern "system" fn oo_set_client_site(this: This, site: This) -> HRESULT {
    let base = base_of(this, OO_SLOT);
    foreign_add_ref(site);
    foreign_release((*base).client_site);
    (*base).client_site = site;
    S_OK
}

unsafe extern "system" fn oo_get_client_site(this: This, out: *mut This) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let base = base_of(this, OO_SLOT);
    foreign_add_ref((*base).client_site);
    *out = (*base).client_site;
    S_OK
}

unsafe extern "system" fn oo_set_host_names(
    _this: This,
    _container: PCWSTR,
    _object: PCWSTR,
) -> HRESULT {
    S_OK
}

unsafe extern "system" fn oo_close(_this: This, _save_option: u32) -> HRESULT {
    S_OK
}

unsafe extern "system" fn oo_set_moniker(_this: This, _which: u32, _moniker: This) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn oo_get_moniker(
    _this: This,
    _assign: u32,
    _which: u32,
    out: *mut This,
) -> HRESULT {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    E_NOTIMPL
}

unsafe extern "system" fn oo_init_from_data(
    _this: This,
    _data: This,
    _creation: i32,
    _reserved: u32,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn oo_get_clipboard_data(
    _this: This,
    _reserved: u32,
    out: *mut This,
) -> HRESULT {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    E_NOTIMPL
}

/// The double-click verb: extract the workbook to a temp file and open it in
/// GenOffice. In-place UI activation is deliberately not implemented.
unsafe extern "system" fn oo_do_verb(
    this: This,
    verb: i32,
    _message: This,
    _active_site: This,
    _index: i32,
    _parent: HWND,
    _position: *const RECT,
) -> HRESULT {
    if verb != OLEIVERB_PRIMARY && verb != OLEIVERB_SHOW && verb != OLEIVERB_OPEN {
        return E_NOTIMPL;
    }
    let base = base_of(this, OO_SLOT);
    if (*base).opened {
        return S_OK;
    }
    if (*base).storage.is_null() {
        return E_FAIL;
    }
    let Some(native) = read_stream((*base).storage, &ole10_native_name()) else {
        return E_FAIL;
    };
    let Some(workbook) = native_workbook_bytes(&native) else {
        return E_FAIL;
    };
    if !open_in_genoffice(workbook) {
        return E_FAIL;
    }
    (*base).opened = true;
    S_OK
}

unsafe extern "system" fn oo_enum_verbs(_this: This, out: *mut This) -> HRESULT {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    // Containers fall back to the default verb set when enumeration fails.
    E_NOTIMPL
}

unsafe extern "system" fn oo_update(_this: This) -> HRESULT {
    S_OK
}

unsafe extern "system" fn oo_is_up_to_date(_this: This) -> HRESULT {
    S_OK
}

unsafe extern "system" fn oo_get_user_class_id(_this: This, out: *mut GUID) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    *out = CLSID_GENOFFICE_SHEET;
    S_OK
}

unsafe extern "system" fn oo_get_user_type(_this: This, _form: u32, out: *mut PWSTR) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let name = alloc_olestr(crate::registry::USER_TYPE_NAME);
    *out = name;
    if name.is_null() { E_OUTOFMEMORY } else { S_OK }
}

unsafe extern "system" fn oo_set_extent(_this: This, _aspect: u32, _size: *const SIZE) -> HRESULT {
    // The object renders at its paste-time size; resize requests are ignored.
    S_OK
}

unsafe extern "system" fn oo_get_extent(this: This, aspect: u32, size: *mut SIZE) -> HRESULT {
    if size.is_null() {
        return E_POINTER;
    }
    if aspect != DVASPECT_CONTENT {
        return DV_E_DVASPECT;
    }
    let base = base_of(this, OO_SLOT);
    (*size).cx = (*base).width_himetric as i32;
    (*size).cy = (*base).height_himetric as i32;
    S_OK
}

unsafe extern "system" fn oo_advise(_this: This, _sink: This, connection: *mut u32) -> HRESULT {
    if !connection.is_null() {
        *connection = 0;
    }
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn oo_unadvise(_this: This, _connection: u32) -> HRESULT {
    OLE_E_NOCONNECTION
}

unsafe extern "system" fn oo_enum_advise(_this: This, out: *mut This) -> HRESULT {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn oo_get_misc_status(_this: This, _aspect: u32, out: *mut u32) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    // No OLEMISC flags: a plain embedded object, not insertable, not
    // activated in place.
    *out = 0;
    S_OK
}

unsafe extern "system" fn oo_set_color_scheme(
    _this: This,
    _palette: *const core::ffi::c_void,
) -> HRESULT {
    E_NOTIMPL
}

// ----------------------------------------------------- IDataObject impls

/// FORMATETC gate shared by GetData and QueryGetData: this object serves
/// CF_ENHMETAFILE, content aspect, through an HGLOBAL.
unsafe fn check_format(format: *const FORMATETC) -> HRESULT {
    if format.is_null() {
        return E_POINTER;
    }
    let format = &*format;
    if format.cfFormat != CF_ENHMETAFILE {
        return DV_E_FORMATETC;
    }
    if format.dwAspect != DVASPECT_CONTENT {
        return DV_E_DVASPECT;
    }
    if format.tymed & (TYMED_HGLOBAL as u32) == 0 {
        return DV_E_TYMED;
    }
    S_OK
}

unsafe extern "system" fn do_get_data(
    this: This,
    format: *const FORMATETC,
    medium: *mut STGMEDIUM,
) -> HRESULT {
    if medium.is_null() {
        return E_POINTER;
    }
    let status = check_format(format);
    if status != S_OK {
        return status;
    }
    let base = base_of(this, DO_SLOT);
    if (*base).storage.is_null() {
        return E_FAIL;
    }
    let Some(pres) = read_stream((*base).storage, &olepres_name()) else {
        return E_FAIL;
    };
    let Some(emf) = olepres_emf_bytes(&pres) else {
        return E_FAIL;
    };
    let handle = hglobal_from(emf);
    if handle.is_null() {
        return E_OUTOFMEMORY;
    }
    (*medium).tymed = TYMED_HGLOBAL as u32;
    (*medium).u.hGlobal = handle;
    (*medium).pUnkForRelease = core::ptr::null_mut();
    S_OK
}

unsafe extern "system" fn do_get_data_here(
    _this: This,
    _format: *const FORMATETC,
    _medium: *mut STGMEDIUM,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn do_query_get_data(_this: This, format: *const FORMATETC) -> HRESULT {
    check_format(format)
}

unsafe extern "system" fn do_get_canonical_format_etc(
    _this: This,
    format_in: *const FORMATETC,
    format_out: *mut FORMATETC,
) -> HRESULT {
    if format_in.is_null() || format_out.is_null() {
        return E_POINTER;
    }
    *format_out = *format_in;
    (*format_out).ptd = core::ptr::null_mut();
    DATA_S_SAMEFORMATETC
}

unsafe extern "system" fn do_set_data(
    _this: This,
    _format: *const FORMATETC,
    _medium: *const STGMEDIUM,
    _release: i32,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn do_enum_format_etc(
    _this: This,
    _direction: u32,
    out: *mut This,
) -> HRESULT {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    E_NOTIMPL
}

unsafe extern "system" fn do_d_advise(
    _this: This,
    _format: *const FORMATETC,
    _flags: u32,
    _sink: This,
    connection: *mut u32,
) -> HRESULT {
    if !connection.is_null() {
        *connection = 0;
    }
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn do_d_unadvise(_this: This, _connection: u32) -> HRESULT {
    OLE_E_NOCONNECTION
}

unsafe extern "system" fn do_enum_d_advise(_this: This, out: *mut This) -> HRESULT {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    OLE_E_ADVISENOTSUPPORTED
}

// -------------------------------------------------------- IClassFactory

unsafe extern "system" fn factory_query_interface(
    this: This,
    riid: *const GUID,
    out: *mut This,
) -> HRESULT {
    if riid.is_null() || out.is_null() {
        return E_POINTER;
    }
    let iid = &*riid;
    if !guid_eq(iid, &IID_IUNKNOWN) && !guid_eq(iid, &IID_ICLASS_FACTORY) {
        *out = core::ptr::null_mut();
        return E_NOINTERFACE;
    }
    (*(this as *const ClassFactory))
        .ref_count
        .fetch_add(1, Ordering::Relaxed);
    *out = this;
    S_OK
}

unsafe extern "system" fn factory_add_ref(this: This) -> u32 {
    (*(this as *const ClassFactory))
        .ref_count
        .fetch_add(1, Ordering::Relaxed)
        + 1
}

unsafe extern "system" fn factory_release(this: This) -> u32 {
    // The factory is a static: the count never frees anything.
    (*(this as *const ClassFactory))
        .ref_count
        .fetch_sub(1, Ordering::Relaxed)
        - 1
}

unsafe extern "system" fn factory_create_instance(
    _this: This,
    outer: This,
    riid: *const GUID,
    out: *mut This,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    *out = core::ptr::null_mut();
    // No aggregation: a non-null outer unknown is refused per contract.
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    let object = OleObject::boxed();
    let status = object_query_interface(object, riid, out);
    // The successful QI holds the caller's reference; drop the creation one
    // (which also frees the object when the interface was not supported).
    object_release(object);
    status
}

unsafe extern "system" fn factory_lock_server(_this: This, _lock: i32) -> HRESULT {
    S_OK
}

// -------------------------------------------------------- storage reading

/// IStorage::OpenStream is vtable slot 4 (IUnknown 0..2, CreateStream 3,
/// OpenStream 4): (pwcsName, reserved1, grfMode, reserved2, ppstm).
unsafe fn open_storage_stream(storage: This, name: &[u16]) -> Option<This> {
    if storage.is_null() {
        return None;
    }
    let open_stream: unsafe extern "system" fn(
        This,
        PCWSTR,
        *const core::ffi::c_void,
        u32,
        u32,
        *mut This,
    ) -> HRESULT = core::mem::transmute(foreign_slot(storage, 4));
    let mut stream: This = core::ptr::null_mut();
    let status = open_stream(
        storage,
        name.as_ptr(),
        core::ptr::null(),
        STGM_READ | STGM_SHARE_EXCLUSIVE,
        0,
        &mut stream,
    );
    if status == S_OK && !stream.is_null() {
        Some(stream)
    } else {
        None
    }
}

/// ISequentialStream::Read is vtable slot 3: (pv, cb, pcbRead).
unsafe fn stream_read_all(stream: This) -> Option<Vec<u8>> {
    let read: unsafe extern "system" fn(This, *mut core::ffi::c_void, u32, *mut u32) -> HRESULT =
        core::mem::transmute(foreign_slot(stream, 3));
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let mut got: u32 = 0;
        let status = read(
            stream,
            chunk.as_mut_ptr().cast(),
            chunk.len() as u32,
            &mut got,
        );
        if status != S_OK && status != S_FALSE {
            return None;
        }
        if got == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..got as usize]);
        if status == S_FALSE {
            break;
        }
    }
    Some(out)
}

/// Opens and drains one stream of the object's storage, releasing it after.
unsafe fn read_stream(storage: This, name: &[u16]) -> Option<Vec<u8>> {
    let stream = open_storage_stream(storage, name)?;
    let bytes = stream_read_all(stream);
    foreign_release(stream);
    bytes
}

// ------------------------------------------------------------ launching

/// Copies bytes into a moveable HGLOBAL for an STGMEDIUM the caller frees.
unsafe fn hglobal_from(bytes: &[u8]) -> HGLOBAL {
    let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
    if handle.is_null() {
        return core::ptr::null_mut();
    }
    let locked = GlobalLock(handle);
    if locked.is_null() {
        GlobalFree(handle);
        return core::ptr::null_mut();
    }
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), locked as *mut u8, bytes.len());
    GlobalUnlock(handle);
    handle
}

/// A CoTaskMemAlloc'd OLESTR copy; the caller frees it with CoTaskMemFree.
unsafe fn alloc_olestr(text: &str) -> PWSTR {
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let out = CoTaskMemAlloc(units.len() * 2) as *mut u16;
    if !out.is_null() {
        core::ptr::copy_nonoverlapping(units.as_ptr(), out, units.len());
    }
    out
}

/// The GenOffice executable relative to this helper:
/// <app>/resources/native/win-clipboard-helper.exe → <app>/GenOffice.exe.
/// None in dev layouts, where there is nothing to launch.
fn genoffice_exe() -> Option<std::path::PathBuf> {
    let helper = std::env::current_exe().ok()?;
    let app_dir = helper.parent()?.parent()?.parent()?;
    let exe = app_dir.join("GenOffice.exe");
    exe.is_file().then_some(exe)
}

/// Writes the workbook to a temp .xlsx and launches GenOffice with it.
fn open_in_genoffice(workbook: &[u8]) -> bool {
    let Some(exe) = genoffice_exe() else {
        return false;
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("genoffice-embed-{stamp}.xlsx"));
    if std::fs::write(&path, workbook).is_err() {
        return false;
    }
    // Never wait on the child: the COM call must return at once while
    // GenOffice keeps running with the document.
    std::process::Command::new(exe).arg(&path).spawn().is_ok()
}

// ---------------------------------------------------------------- server

/// Registers the class object and pumps the STA message loop until the
/// process is terminated.
pub fn run_server() -> Result<(), String> {
    unsafe {
        let init = CoInitializeEx(core::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        if init < 0 && init != RPC_E_CHANGED_MODE {
            return Err(format!("CoInitializeEx failed: {init:#x}"));
        }
        let mut cookie: u32 = 0;
        let status = CoRegisterClassObject(
            &CLSID_GENOFFICE_SHEET,
            core::ptr::addr_of!(CLASS_FACTORY) as This,
            CLSCTX_LOCAL_SERVER,
            REGCLS_MULTIPLEUSE as u32,
            &mut cookie,
        );
        if status < 0 {
            return Err(format!("CoRegisterClassObject failed: {status:#x}"));
        }
        let mut message: MSG = core::mem::zeroed();
        // GetMessageW returns 0 on WM_QUIT and -1 on error; both end the loop.
        while GetMessageW(&mut message, core::ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&message);
        }
        CoRevokeClassObject(cookie);
    }
    Ok(())
}
