//! The part of mpv's client API (`client.h`) and render API (`render.h`)
//! this crate calls, resolved from the library at run time.

use crate::Error;
use std::ffi::{CStr, c_char, c_double, c_int, c_ulong, c_void};
use std::path::Path;

/// `mpv_handle`.
pub(crate) type Handle = c_void;
/// `mpv_render_context`.
pub(crate) type RenderContext = c_void;

/// `mpv_format`.
pub(crate) const FORMAT_NONE: c_int = 0;
pub(crate) const FORMAT_FLAG: c_int = 3;
pub(crate) const FORMAT_INT64: c_int = 4;
pub(crate) const FORMAT_DOUBLE: c_int = 5;

/// `mpv_event_id`.
pub(crate) const EVENT_NONE: c_int = 0;
pub(crate) const EVENT_SHUTDOWN: c_int = 1;
pub(crate) const EVENT_END_FILE: c_int = 7;
pub(crate) const EVENT_FILE_LOADED: c_int = 8;
pub(crate) const EVENT_PROPERTY_CHANGE: c_int = 22;
pub(crate) const EVENT_HOOK: c_int = 25;

/// `mpv_end_file_reason`.
pub(crate) const END_FILE_ERROR: c_int = 4;

/// `mpv_render_param_type`.
pub(crate) const RENDER_PARAM_INVALID: c_int = 0;
pub(crate) const RENDER_PARAM_API_TYPE: c_int = 1;
pub(crate) const RENDER_PARAM_SW_SIZE: c_int = 17;
pub(crate) const RENDER_PARAM_SW_FORMAT: c_int = 18;
pub(crate) const RENDER_PARAM_SW_STRIDE: c_int = 19;
pub(crate) const RENDER_PARAM_SW_POINTER: c_int = 20;
/// `MPV_RENDER_API_TYPE_SW`.
pub(crate) const RENDER_API_SW: &CStr = c"sw";
/// `MPV_RENDER_UPDATE_FRAME`.
pub(crate) const RENDER_UPDATE_FRAME: u64 = 1;

/// `mpv_render_param`.
#[repr(C)]
pub(crate) struct RenderParam {
    pub kind: c_int,
    pub data: *mut c_void,
}

/// `mpv_event`.
#[repr(C)]
pub(crate) struct Event {
    pub id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

/// `mpv_event_property`.
#[repr(C)]
pub(crate) struct EventProperty {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

/// `mpv_event_hook`.
#[repr(C)]
pub(crate) struct EventHook {
    pub name: *const c_char,
    pub id: u64,
}

/// The leading fields of `mpv_event_end_file`.
#[repr(C)]
pub(crate) struct EventEndFile {
    pub reason: c_int,
    pub error: c_int,
}

pub(crate) type Callback = unsafe extern "C" fn(*mut c_void);

macro_rules! api {
    ($( $name:ident : fn($($arg:ty),*) $(-> $ret:ty)? ; )*) => {
        /// libmpv's functions. The library stays loaded while this lives.
        pub(crate) struct Api {
            $( pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?, )*
            _library: libloading::Library,
        }
        impl Api {
            fn resolve(library: libloading::Library) -> Result<Self, Error> {
                // SAFETY: each symbol is given the signature mpv's headers declare for it.
                unsafe {
                    Ok(Self {
                        $( $name: *library
                            .get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(
                                concat!(stringify!($name), "\0").as_bytes(),
                            )
                            .map_err(|e| Error::Load(format!("{}: {e}", stringify!($name))))?, )*
                        _library: library,
                    })
                }
            }
        }
    };
}

api! {
    mpv_client_api_version: fn() -> c_ulong;
    mpv_error_string: fn(c_int) -> *const c_char;
    mpv_free: fn(*mut c_void);
    mpv_create: fn() -> *mut Handle;
    mpv_initialize: fn(*mut Handle) -> c_int;
    mpv_terminate_destroy: fn(*mut Handle);
    mpv_set_option_string: fn(*mut Handle, *const c_char, *const c_char) -> c_int;
    mpv_command: fn(*mut Handle, *mut *const c_char) -> c_int;
    mpv_command_async: fn(*mut Handle, u64, *mut *const c_char) -> c_int;
    mpv_get_property_string: fn(*mut Handle, *const c_char) -> *mut c_char;
    mpv_observe_property: fn(*mut Handle, u64, *const c_char, c_int) -> c_int;
    mpv_wait_event: fn(*mut Handle, c_double) -> *mut Event;
    mpv_set_wakeup_callback: fn(*mut Handle, Option<Callback>, *mut c_void);
    mpv_hook_add: fn(*mut Handle, u64, *const c_char, c_int) -> c_int;
    mpv_hook_continue: fn(*mut Handle, u64) -> c_int;
    mpv_render_context_create: fn(*mut *mut RenderContext, *mut Handle, *mut RenderParam) -> c_int;
    mpv_render_context_set_update_callback: fn(*mut RenderContext, Option<Callback>, *mut c_void);
    mpv_render_context_update: fn(*mut RenderContext) -> u64;
    mpv_render_context_render: fn(*mut RenderContext, *mut RenderParam) -> c_int;
    mpv_render_context_free: fn(*mut RenderContext);
}

impl Api {
    /// Loads the library at the absolute `path` and nothing found by a search.
    pub(crate) fn load(path: &Path) -> Result<Self, Error> {
        debug_assert!(path.is_absolute());
        let load = |e: libloading::Error| Error::Load(format!("{}: {e}", path.display()));
        #[cfg(windows)]
        let library: libloading::Library = {
            use libloading::os::windows::{
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, Library,
            };
            // The DLLs it imports come from its own folder and System32 only: not the
            // game's folder, the working directory or PATH.
            let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
            // SAFETY: loading runs the library's initializers; the caller checked which
            // file this is (`library::verified`) and the player trusted the native mod.
            unsafe { Library::load_with_flags(path, flags) }
                .map_err(load)?
                .into()
        };
        #[cfg(not(windows))]
        // SAFETY: as above. A path with a slash is opened as given, never searched for.
        let library = unsafe { libloading::Library::new(path) }.map_err(load)?;
        Self::resolve(library)
    }

    // `unsigned long` is 32 bits on Windows and 64 elsewhere.
    #[allow(clippy::unnecessary_cast)]
    pub(crate) fn client_api_version(&self) -> u32 {
        // SAFETY: takes nothing, returns a number.
        unsafe { (self.mpv_client_api_version)() as u32 }
    }

    /// mpv's text for the error `code`.
    pub(crate) fn error(&self, code: c_int) -> String {
        // SAFETY: mpv returns a static string for every code.
        unsafe {
            let text = (self.mpv_error_string)(code);
            if text.is_null() {
                format!("error {code}")
            } else {
                CStr::from_ptr(text).to_string_lossy().into_owned()
            }
        }
    }

    /// `Err` with mpv's text when `code` is an error.
    pub(crate) fn check(&self, call: &'static str, code: c_int) -> Result<(), Error> {
        if code < 0 {
            Err(Error::Mpv(call, self.error(code)))
        } else {
            Ok(())
        }
    }
}
