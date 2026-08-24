use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd},
};

const BPF_PROG_TEST_RUN: libc::c_int = 10;
const OUTPUT_CAPACITY: usize = 4096;

pub const XDP_ABORTED: u32 = 0;
pub const XDP_DROP: u32 = 1;
pub const XDP_PASS: u32 = 2;
pub const XDP_TX: u32 = 3;
pub const XDP_REDIRECT: u32 = 4;

#[repr(C)]
#[derive(Default)]
struct TestAttr {
    prog_fd: u32,
    retval: u32,
    data_size_in: u32,
    data_size_out: u32,
    data_in: u64,
    data_out: u64,
    repeat: u32,
    duration: u32,
    ctx_size_in: u32,
    ctx_size_out: u32,
    ctx_in: u64,
    ctx_out: u64,
    flags: u32,
    cpu: u32,
    batch_size: u32,
    pad: u32,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub verdict: u32,
    pub packet: Vec<u8>,
    pub duration_ns: u32,
}

impl Outcome {
    pub fn verdict_name(&self) -> &'static str {
        match self.verdict {
            XDP_ABORTED => "XDP_ABORTED",
            XDP_DROP => "XDP_DROP",
            XDP_PASS => "XDP_PASS",
            XDP_TX => "XDP_TX",
            XDP_REDIRECT => "XDP_REDIRECT",
            _ => "unknown",
        }
    }
}

pub fn run(program: BorrowedFd<'_>, packet: &[u8], repeat: u32) -> io::Result<Outcome> {
    let mut output = vec![0u8; OUTPUT_CAPACITY];

    let mut attr = TestAttr {
        prog_fd: program.as_raw_fd() as u32,
        data_size_in: packet.len() as u32,
        data_size_out: output.len() as u32,
        data_in: packet.as_ptr() as u64,
        data_out: output.as_mut_ptr() as u64,
        repeat: repeat.max(1),
        ..Default::default()
    };

    let ret = unsafe {
        libc::syscall(
            libc::SYS_bpf,
            BPF_PROG_TEST_RUN,
            &mut attr as *mut TestAttr,
            std::mem::size_of::<TestAttr>() as libc::c_uint,
        )
    };

    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    output.truncate(attr.data_size_out as usize);

    Ok(Outcome {
        verdict: attr.retval,
        packet: output,
        duration_ns: attr.duration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attr_matches_the_kernel_layout() {
        assert_eq!(std::mem::size_of::<TestAttr>(), 80);
        assert_eq!(std::mem::align_of::<TestAttr>(), 8);
    }
}
