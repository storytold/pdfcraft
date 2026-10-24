use crate::bindings::pdfium::*;
use std::convert::AsMut;
use std::convert::TryInto;
use std::ffi::c_void;
use std::io;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::marker::PhantomData;
use std::os::raw::c_int;
use std::os::raw::c_uchar;
use std::os::raw::c_ulong;
pub struct PdfiumCustomDocument<'a, T>
where
    T: 'a + Read + Seek,
{
    phantom: PhantomData<&'a mut T>,
    access: FPDF_FILEACCESS,
}

impl<'a, T> PdfiumCustomDocument<'a, T>
where
    T: 'a + Read + Seek,
{
    pub fn new(inner: &'a mut T) -> io::Result<Self> {
        Ok(Self {
            access: FPDF_FILEACCESS {
                m_FileLen: inner
                    .seek(SeekFrom::End(0))?
                    .try_into()
                    .map_err(|x| io::Error::new(io::ErrorKind::Other, x))?,
                m_GetBlock: Some(Self::get_block),
                m_Param: inner as *mut T as *mut c_void,
            },
            phantom: PhantomData,
        })
    }

    unsafe extern "C" fn get_block(
        param: *mut c_void,
        position: c_ulong,
        p_buf: *mut c_uchar,
        size: c_ulong,
    ) -> c_int {
        let file = &mut *(param as *mut T);
        let buf = std::slice::from_raw_parts_mut(p_buf, size as usize);
        file.seek(SeekFrom::Start(position.into()))
            .and_then(|_| file.read_exact(buf))
            .is_ok()
            .into()
    }
}

impl<'a, T> AsMut<FPDF_FILEACCESS> for PdfiumCustomDocument<'a, T>
where
    T: 'a + Read + Seek,
{
    fn as_mut(&mut self) -> &mut FPDF_FILEACCESS {
        &mut self.access
    }
}
