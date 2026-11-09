use windows::Win32::System::Com::{IStream, STREAM_SEEK_END, STREAM_SEEK_SET};

pub fn read_com_stream(stream: &IStream) -> Result<Vec<u8>, windows::core::Error> {
    let mut size = 0;
    unsafe {
        stream.Seek(0, STREAM_SEEK_END, Some(&mut size))?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let mut data = Vec::with_capacity(size as usize);
        let mut n_read_bytes = 0;
        stream
            .Read(
                data.as_mut_ptr() as *mut _,
                data.capacity() as u32,
                Some(&mut n_read_bytes),
            )
            .ok()?;
        data.set_len(n_read_bytes as usize);
        Ok(data)
    }
}

/// Copies the contents of a COM stream to a vector.
pub fn copy_com_stream_to_vec(
    dest: &mut Vec<u8>,
    stream: &IStream,
) -> Result<(), windows::core::Error> {
    let mut size = 0;
    unsafe {
        stream.Seek(0, STREAM_SEEK_END, Some(&mut size))?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        dest.clear();
        dest.reserve(size as usize);
        let mut n_read_bytes = 0;
        stream
            .Read(
                dest.as_mut_ptr() as *mut _,
                dest.capacity() as u32,
                Some(&mut n_read_bytes),
            )
            .ok()?;
        dest.set_len(n_read_bytes as usize);
        Ok(())
    }
}
