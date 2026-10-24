#![doc = include_str!("../readme.md")]
#![no_std]

/// Defines an external function to import.
///
/// PdfCraft patch (Windows 7, `i686-win7-windows-msvc`): the COM allocator and the free-threaded
/// marshaler are imported from `ole32.dll`, which has them on every Windows version (on Windows 8
/// and later it forwards them to combase), not from `combase.dll`, which Windows 7 lacks. The
/// redirect has to happen here, for every crate at once: on x86 the import symbols are decorated
/// the same whichever DLL a crate names (`__imp__CoTaskMemFree@4`), so the linker keeps a single
/// one, and a combase binding in `windows-sys` 0.61 or `windows` 0.62 won over windows-core's
/// ole32 one. See PdfCraft's vendor/README.md.
#[cfg(all(windows, target_arch = "x86"))]
#[macro_export]
macro_rules! link {
    ("combase.dll" $abi:literal $($link_name:literal)? fn CoTaskMemAlloc $($rest:tt)*) => (
        $crate::link!("ole32.dll" $abi $($link_name)? fn CoTaskMemAlloc $($rest)*);
    );
    ("combase.dll" $abi:literal $($link_name:literal)? fn CoTaskMemFree $($rest:tt)*) => (
        $crate::link!("ole32.dll" $abi $($link_name)? fn CoTaskMemFree $($rest)*);
    );
    ("combase.dll" $abi:literal $($link_name:literal)? fn CoTaskMemRealloc $($rest:tt)*) => (
        $crate::link!("ole32.dll" $abi $($link_name)? fn CoTaskMemRealloc $($rest)*);
    );
    ("combase.dll" $abi:literal $($link_name:literal)? fn CoCreateFreeThreadedMarshaler $($rest:tt)*) => (
        $crate::link!("ole32.dll" $abi $($link_name)? fn CoCreateFreeThreadedMarshaler $($rest)*);
    );
    ($library:literal $abi:literal $($link_name:literal)? fn $($function:tt)*) => (
        #[link(name = $library, kind = "raw-dylib", modifiers = "+verbatim", import_name_type = "undecorated")]
        extern $abi {
            $(#[link_name=$link_name])?
            pub fn $($function)*;
        }
    )
}

/// Defines an external function to import.
///
/// PdfCraft patch (Windows 7, `x86_64-win7-windows-msvc`): the same `ole32.dll` redirect as on
/// x86. On x64 no linker collision hides windows-core's binding, but `windows-sys` 0.61 and
/// `windows` 0.62 would each import combase.dll, which Windows 7 lacks.
#[cfg(all(windows, not(target_arch = "x86")))]
#[macro_export]
macro_rules! link {
    ("combase.dll" $abi:literal $($link_name:literal)? fn CoTaskMemAlloc $($rest:tt)*) => (
        $crate::link!("ole32.dll" $abi $($link_name)? fn CoTaskMemAlloc $($rest)*);
    );
    ("combase.dll" $abi:literal $($link_name:literal)? fn CoTaskMemFree $($rest:tt)*) => (
        $crate::link!("ole32.dll" $abi $($link_name)? fn CoTaskMemFree $($rest)*);
    );
    ("combase.dll" $abi:literal $($link_name:literal)? fn CoTaskMemRealloc $($rest:tt)*) => (
        $crate::link!("ole32.dll" $abi $($link_name)? fn CoTaskMemRealloc $($rest)*);
    );
    ("combase.dll" $abi:literal $($link_name:literal)? fn CoCreateFreeThreadedMarshaler $($rest:tt)*) => (
        $crate::link!("ole32.dll" $abi $($link_name)? fn CoCreateFreeThreadedMarshaler $($rest)*);
    );
    ($library:literal $abi:literal $($link_name:literal)? fn $($function:tt)*) => (
        #[link(name = $library, kind = "raw-dylib", modifiers = "+verbatim")]
        extern $abi {
            $(#[link_name=$link_name])?
            pub fn $($function)*;
        }
    )
}

/// Defines an external function to import.
#[cfg(not(windows))]
#[macro_export]
macro_rules! link {
    ($library:literal $abi:literal $($link_name:literal)? fn $($function:tt)*) => (
        extern $abi {
            pub fn $($function)*;
        }
    )
}
