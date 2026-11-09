use crate::utils::com::ComInitializer;
use scopeguard::defer;
use std::mem;
use std::ptr;
use std::slice;
use thiserror::Error;
use windows::Win32::Graphics::Gdi::MODIFY_WORLD_TRANSFORM_MODE;
use windows::Win32::Graphics::Gdi::{CHECKJPEGFORMAT, CHECKPNGFORMAT, EMRSETWORLDTRANSFORM};
use windows::Win32::Graphics::Gdi::{EMRMODIFYWORLDTRANSFORM, QUERYESCSUPPORT};
use windows::Win32::Storage::Xps::ExtEscape;
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{LPARAM, RECT},
        Graphics::{
            Gdi::{
                CloseEnhMetaFile, CreateEnhMetaFileW, DeleteEnhMetaFile, EnumEnhMetaFile,
                GetEnhMetaFileHeader, GetWorldTransform, ModifyWorldTransform,
                PlayEnhMetaFileRecord, SetWorldTransform, StretchDIBits, BITMAPINFO,
                BITMAPINFOHEADER, BI_JPEG, BI_PNG, BI_RGB, DIB_USAGE, EMRSTRETCHDIBITS,
                EMR_MODIFYWORLDTRANSFORM, EMR_SETLAYOUT, EMR_SETWORLDTRANSFORM, EMR_STRETCHDIBITS,
                ENHMETAHEADER, ENHMETARECORD, HANDLETABLE, HDC, HENHMETAFILE, MWT_IDENTITY,
                MWT_LEFTMULTIPLY, MWT_RIGHTMULTIPLY, ROP_CODE, XFORM,
            },
            Imaging::{
                CLSID_WICImagingFactory2, GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone,
                WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
                D2D::IWICImagingFactory2,
            },
        },
        System::Com::{CoCreateInstance, CLSCTX_ALL},
    },
};

#[derive(Error, Debug)]
#[error("Failed to create EMF")]
pub struct EmfCreateFailedError;

#[derive(Error, Debug)]
#[error("Failed to play EMF")]
pub struct EmfPlaybackFailedError;

pub struct Emf {
    metafile: HENHMETAFILE,
}

impl Emf {
    /// Creates a new EMF by drawing on a metafile DC.
    /// All units are in .01-millimeter units.
    pub unsafe fn new(
        hdc: HDC,
        width: i32,
        height: i32,
        draw: impl FnOnce(HDC) -> bool,
    ) -> Result<Self, EmfCreateFailedError> {
        // in .01-millimeter units
        let frame = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };

        let emf_hdc = CreateEnhMetaFileW(
            Some(hdc),
            PCWSTR::null(),
            Some(ptr::addr_of!(frame)),
            PCWSTR::null(),
        );
        if emf_hdc.is_invalid() {
            return Err(EmfCreateFailedError);
        }

        if !draw(emf_hdc) {
            let _ = CloseEnhMetaFile(emf_hdc);
            return Err(EmfCreateFailedError);
        }

        let metafile = CloseEnhMetaFile(emf_hdc);
        if metafile.is_invalid() {
            return Err(EmfCreateFailedError);
        }

        Ok(Self { metafile })
    }

    pub unsafe fn playback(
        &self,
        printer_hdc: HDC,
        rect: RECT,
    ) -> Result<(), EmfPlaybackFailedError> {
        let mut base_matrix = XFORM::default();
        if !GetWorldTransform(printer_hdc, &mut base_matrix).as_bool() {
            return Err(EmfPlaybackFailedError);
        }
        defer! {
            let _ = SetWorldTransform(printer_hdc, ptr::addr_of!(base_matrix));
        }

        let mut context = EnumerationContext { base_matrix };

        let mut header = ENHMETAHEADER::default();
        if GetEnhMetaFileHeader(
            self.metafile,
            mem::size_of::<ENHMETAHEADER>() as u32,
            Some(&mut header),
        ) != mem::size_of::<ENHMETAHEADER>() as u32
        {
            return Err(EmfPlaybackFailedError);
        }
        if EnumEnhMetaFile(
            Some(printer_hdc),
            self.metafile,
            Some(Self::playback_proc),
            Some((&mut context as *mut EnumerationContext).cast()),
            Some(ptr::addr_of!(rect)),
        )
        .as_bool()
        {
            return Ok(());
        }
        Err(EmfPlaybackFailedError)
    }

    unsafe extern "system" fn playback_proc(
        hdc: HDC,
        handle_table: *const HANDLETABLE,
        record: *const ENHMETARECORD,
        handle_count: i32,
        param: LPARAM,
    ) -> i32 {
        let context = &mut *(param.0 as *mut EnumerationContext);
        let handle_table_slice = if handle_table.is_null() || handle_count <= 0 {
            &[]
        } else {
            slice::from_raw_parts(handle_table, handle_count as usize)
        };
        match play_record(hdc, record, handle_table_slice, context) {
            true => 1,
            false => 0,
        }
    }
}

impl Drop for Emf {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteEnhMetaFile(Some(self.metafile));
        }
    }
}

struct EnumerationContext {
    base_matrix: XFORM,
}

unsafe fn play_record(
    hdc: HDC,
    record: *const ENHMETARECORD,
    handle_table_slice: &[HANDLETABLE],
    context: &mut EnumerationContext,
) -> bool {
    match (*record).iType {
        EMR_STRETCHDIBITS => {
            // EMRSTRETCHDIBITS is flexible in size, so we check the size with "<" operation.
            if (*record).nSize < (mem::size_of::<EMRSTRETCHDIBITS>()) as u32 {
                return false;
            }
            play_stretch_dibits_record(hdc, record as *const EMRSTRETCHDIBITS, handle_table_slice)
        }
        EMR_SETWORLDTRANSFORM => {
            if (*record).nSize != (mem::size_of::<EMRSETWORLDTRANSFORM>()) as u32 {
                return false;
            }
            let set_world_transform = &*(record as *const EMRSETWORLDTRANSFORM);
            SetWorldTransform(hdc, ptr::addr_of!(context.base_matrix)).as_bool()
                && ModifyWorldTransform(
                    hdc,
                    Some(ptr::addr_of!(set_world_transform.xform)),
                    MWT_LEFTMULTIPLY,
                )
                .as_bool()
        }
        EMR_MODIFYWORLDTRANSFORM => {
            if (*record).nSize != (mem::size_of::<EMRMODIFYWORLDTRANSFORM>()) as u32 {
                return false;
            }
            let modify_world_transform = &*(record as *const EMRMODIFYWORLDTRANSFORM);
            match modify_world_transform.iMode {
                MWT_IDENTITY => {
                    SetWorldTransform(hdc, ptr::addr_of!(context.base_matrix)).as_bool()
                }
                MWT_LEFTMULTIPLY => ModifyWorldTransform(
                    hdc,
                    Some(ptr::addr_of!(modify_world_transform.xform)),
                    MWT_LEFTMULTIPLY,
                )
                .as_bool(),
                MWT_RIGHTMULTIPLY => ModifyWorldTransform(
                    hdc,
                    Some(ptr::addr_of!(modify_world_transform.xform)),
                    MWT_RIGHTMULTIPLY,
                )
                .as_bool(),
                // 4 = MWT_SET
                MODIFY_WORLD_TRANSFORM_MODE(4) => {
                    SetWorldTransform(hdc, ptr::addr_of!(context.base_matrix)).as_bool()
                        && ModifyWorldTransform(
                            hdc,
                            Some(ptr::addr_of!(modify_world_transform.xform)),
                            MWT_LEFTMULTIPLY,
                        )
                        .as_bool()
                }
                _ => false,
            }
        }
        // Skip EMR_SETLAYOUT because some printer drivers choke on
        // layout changes when replaying EMF records.
        // Chromium also skips EMR_SETLAYOUT in their EMF playback implementation:
        // https://github.com/chromium/chromium/blob/056cc5e47e8dd311cf345d4d83a2729691ac2610/printing/emf_win.cc#L368-L371
        EMR_SETLAYOUT => true,
        _ => PlayEnhMetaFileRecord(hdc, handle_table_slice, record).as_bool(),
    }
}

unsafe fn play_stretch_dibits_record(
    hdc: HDC,
    sdib_record: *const EMRSTRETCHDIBITS,
    handle_table_slice: &[HANDLETABLE],
) -> bool {
    let record_size = (*sdib_record).emr.nSize as usize;
    let bmi_offset = (*sdib_record).offBmiSrc as usize;
    let bmi_len = (*sdib_record).cbBmiSrc as usize;
    let bmi_end = match bmi_offset.checked_add(bmi_len) {
        Some(end) => end,
        None => return false,
    };
    if bmi_len < mem::size_of::<BITMAPINFOHEADER>() || bmi_end > record_size {
        return false;
    }
    let bitmap_header = &*(sdib_record.byte_add(bmi_offset) as *const BITMAPINFOHEADER);
    let bits_offset = (*sdib_record).offBitsSrc as usize;
    let bits_len = bitmap_header.biSizeImage as usize;
    let bits_end = match bits_offset.checked_add(bits_len) {
        Some(end) => end,
        None => return false,
    };
    if bits_end > record_size {
        return false;
    }
    let bits = {
        if bits_len == 0 {
            &[]
        } else {
            slice::from_raw_parts(sdib_record.byte_add(bits_offset) as *const u8, bits_len)
        }
    };
    if (bitmap_header.biCompression == BI_JPEG.0
        && !dib_format_natively_supported(hdc, CHECKJPEGFORMAT, bits))
        || (bitmap_header.biCompression == BI_PNG.0
            && !dib_format_natively_supported(hdc, CHECKPNGFORMAT, bits))
    {
        // Decode compressed format in application and play with StretchDIBits, to avoid EMF playback failure.
        // Aligns with how Chromium plays EMF records with compressed DIBs:
        // https://github.com/chromium/chromium/blob/c6e8aae4573cec06352a4748ae902831f9a1334c/printing/emf_win.cc#L276
        return match DecodedBitmap::from_compressed(bits) {
            Ok(decoded) => {
                StretchDIBits(
                    hdc,
                    (*sdib_record).xDest,
                    (*sdib_record).yDest,
                    (*sdib_record).cxDest,
                    (*sdib_record).cyDest,
                    (*sdib_record).xSrc,
                    (*sdib_record).ySrc,
                    (*sdib_record).cxSrc,
                    (*sdib_record).cySrc,
                    Some(decoded.pixels.as_ptr().cast()),
                    ptr::addr_of!(decoded.bitmap_info),
                    DIB_USAGE((*sdib_record).iUsageSrc),
                    ROP_CODE((*sdib_record).dwRop),
                ) != 0
            }
            Err(_) => false,
        };
    }

    PlayEnhMetaFileRecord(hdc, handle_table_slice, sdib_record as *const ENHMETARECORD).as_bool()
}

fn dib_format_natively_supported(dc: HDC, escape: u32, bits: &[u8]) -> bool {
    let mut supported: [u8; 4] = [0, 0, 0, 0];
    unsafe {
        if ExtEscape(
            dc,
            QUERYESCSUPPORT as _,
            Some(escape.to_ne_bytes().as_slice()),
            None,
        ) > 0
        {
            ExtEscape(dc, escape as _, Some(bits), Some(supported.as_mut_slice()));
        }
    }
    supported != [0, 0, 0, 0]
}

struct DecodedBitmap {
    bitmap_info: BITMAPINFO,
    pixels: Vec<u8>,
}

impl DecodedBitmap {
    unsafe fn from_compressed(bytes: &[u8]) -> windows::core::Result<Self> {
        let _com = ComInitializer::new();
        let factory: IWICImagingFactory2 =
            CoCreateInstance(&CLSID_WICImagingFactory2, None, CLSCTX_ALL)?;
        let stream = factory.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder = factory.CreateDecoderFromStream(
            &stream,
            ptr::null(),
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeMedianCut,
        )?;

        let mut width = 0u32;
        let mut height = 0u32;
        converter.GetSize(&mut width, &mut height)?;
        let stride = width.saturating_mul(4);
        let mut pixels = vec![0u8; stride.saturating_mul(height) as usize];
        converter.CopyPixels(ptr::null(), stride, &mut pixels)?;

        Ok(Self {
            bitmap_info: BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: pixels.len() as u32,
                    ..Default::default()
                },
                ..Default::default()
            },
            pixels,
        })
    }
}
