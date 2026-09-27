//! A read-only `IStream` over a shared document buffer, so a response of up
//! to the 128 MiB page limit is served without a per-request copy.

use std::{
    ffi::c_void,
    sync::{Arc, Mutex, PoisonError},
};

use windows::{
    Win32::{
        Foundation::{
            E_NOTIMPL, S_OK, STG_E_ACCESSDENIED, STG_E_INVALIDFUNCTION, STG_E_INVALIDPOINTER,
        },
        System::Com::{
            ISequentialStream_Impl, IStream, IStream_Impl, LOCKTYPE, STATFLAG, STATSTG, STGC,
            STGM_READ, STGM_SHARE_DENY_WRITE, STGTY_STREAM, STREAM_SEEK, STREAM_SEEK_CUR,
            STREAM_SEEK_END, STREAM_SEEK_SET,
        },
    },
    core::{HRESULT, Ref, implement},
};

/// Free-threaded (windows-rs implementations are agile) because WebView2 may
/// read response content off the UI thread; the buffer is immutable and the
/// cursor is behind a mutex.
#[implement(IStream)]
pub(crate) struct SharedStream {
    bytes: Arc<Vec<u8>>,
    position: Mutex<u64>,
}

impl SharedStream {
    pub(crate) fn create(bytes: Arc<Vec<u8>>) -> IStream {
        Self::at(bytes, 0)
    }

    fn at(bytes: Arc<Vec<u8>>, position: u64) -> IStream {
        Self {
            bytes,
            position: Mutex::new(position),
        }
        .into()
    }
}

impl ISequentialStream_Impl for SharedStream_Impl {
    fn Read(&self, pv: *mut c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        if pv.is_null() {
            return STG_E_INVALIDPOINTER;
        }
        let mut position = self.position.lock().unwrap_or_else(PoisonError::into_inner);
        let length = self.bytes.len();
        let start = usize::try_from(*position).unwrap_or(usize::MAX).min(length);
        let count = (cb as usize).min(length - start);
        // SAFETY: the caller supplies `pv` valid for `cb` bytes; `count <= cb`
        // and `start + count <= length`, and the regions cannot overlap
        // because the source is this stream's private immutable buffer.
        unsafe {
            std::ptr::copy_nonoverlapping(self.bytes.as_ptr().add(start), pv.cast::<u8>(), count);
        }
        *position = (start + count) as u64;
        if !pcbread.is_null() {
            // SAFETY: a non-null `pcbread` is a caller-owned u32 out pointer.
            unsafe { *pcbread = count as u32 };
        }
        // Like CreateStreamOnHGlobal: a short read at the end is S_OK.
        S_OK
    }

    fn Write(&self, _pv: *const c_void, _cb: u32, _pcbwritten: *mut u32) -> HRESULT {
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for SharedStream_Impl {
    fn Seek(
        &self,
        dlibmove: i64,
        dworigin: STREAM_SEEK,
        plibnewposition: *mut u64,
    ) -> windows::core::Result<()> {
        let mut position = self.position.lock().unwrap_or_else(PoisonError::into_inner);
        let base = match dworigin {
            STREAM_SEEK_SET => 0_i128,
            STREAM_SEEK_CUR => i128::from(*position),
            STREAM_SEEK_END => self.bytes.len() as i128,
            _ => return Err(STG_E_INVALIDFUNCTION.into()),
        };
        let target = base + i128::from(dlibmove);
        let target = u64::try_from(target).map_err(|_| STG_E_INVALIDFUNCTION)?;
        *position = target;
        if !plibnewposition.is_null() {
            // SAFETY: a non-null out pointer is a caller-owned u64.
            unsafe { *plibnewposition = target };
        }
        Ok(())
    }

    fn SetSize(&self, _libnewsize: u64) -> windows::core::Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }

    fn CopyTo(
        &self,
        _pstm: Ref<IStream>,
        _cb: u64,
        _pcbread: *mut u64,
        _pcbwritten: *mut u64,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Commit(&self, _grfcommitflags: &STGC) -> windows::core::Result<()> {
        Ok(())
    }

    fn Revert(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn LockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: &LOCKTYPE,
    ) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn UnlockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: u32,
    ) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn Stat(&self, pstatstg: *mut STATSTG, _grfstatflag: &STATFLAG) -> windows::core::Result<()> {
        if pstatstg.is_null() {
            return Err(STG_E_INVALIDPOINTER.into());
        }
        // SAFETY: a non-null `pstatstg` is a caller-owned STATSTG. No name is
        // allocated, so `pwcsName` stays null whatever the flag requests.
        unsafe {
            *pstatstg = STATSTG {
                r#type: STGTY_STREAM.0 as u32,
                cbSize: self.bytes.len() as u64,
                grfMode: STGM_READ | STGM_SHARE_DENY_WRITE,
                ..Default::default()
            };
        }
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IStream> {
        let position = *self.position.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(SharedStream::at(Arc::clone(&self.bytes), position))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use windows::Win32::System::Com::{STATFLAG_NONAME, STATSTG, STREAM_SEEK_END, STREAM_SEEK_SET};

    use super::SharedStream;

    #[test]
    fn reads_seeks_and_stats_the_shared_buffer_without_copying_it() {
        let bytes = Arc::new((0..=255_u8).cycle().take(10_000).collect::<Vec<_>>());
        let stream = SharedStream::create(Arc::clone(&bytes));
        assert_eq!(Arc::strong_count(&bytes), 2, "the stream shares the buffer");
        let mut out = vec![0_u8; 4096];
        let mut read = 0;
        let mut total = Vec::new();
        loop {
            // SAFETY: `out` is valid for its length; `read` is a live local.
            unsafe { stream.Read(out.as_mut_ptr().cast(), out.len() as u32, Some(&mut read)) }
                .ok()
                .unwrap();
            if read == 0 {
                break;
            }
            total.extend_from_slice(&out[..read as usize]);
        }
        assert_eq!(total, *bytes);
        let mut position = 0;
        unsafe { stream.Seek(-10, STREAM_SEEK_END, Some(&mut position)) }.unwrap();
        assert_eq!(position, 9_990);
        unsafe { stream.Read(out.as_mut_ptr().cast(), 100, Some(&mut read)) }
            .ok()
            .unwrap();
        assert_eq!(&out[..read as usize], &bytes[9_990..]);
        assert!(unsafe { stream.Seek(-1, STREAM_SEEK_SET, None) }.is_err());
        let mut stat = STATSTG::default();
        unsafe { stream.Stat(&mut stat, STATFLAG_NONAME) }.unwrap();
        assert_eq!(stat.cbSize, 10_000);
        assert!(stat.pwcsName.is_null());
        assert!(unsafe { stream.Write(out.as_ptr().cast(), 1, None) }.is_err());
        drop(stream);
        assert_eq!(Arc::strong_count(&bytes), 1);
    }
}
