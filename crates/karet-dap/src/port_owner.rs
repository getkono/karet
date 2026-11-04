//! Who listens on a spawn-then-TCP adapter's port: the adapter's own process
//! tree, or some other process that took the port between the client choosing
//! it and the adapter binding it.
//!
//! Read from the kernel's socket table through the OS's own interfaces:
//! Linux's `/proc`, macOS's `proc_pidinfo`/`proc_pidfdinfo` (through `libc`),
//! and Windows' `GetExtendedTcpTable` with a Toolhelp process snapshot
//! (through `windows-sys`). Elsewhere the owner is [`PortOwner::Unknown`] and
//! the launch falls back to watching the adapter's exit.
//!
//! On macOS and Windows the reader first proves itself once per process: it
//! binds a listener of its own and must find it held by this process. A
//! reader that cannot (an OS whose structures moved) reports
//! [`PortOwner::Unknown`] rather than a wrong answer, so the launch degrades
//! to the exit-based check instead of waiting on a port it misreads.

/// What the socket table says about the listeners on a loopback port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortOwner {
    /// Nothing listens on the port yet.
    Vacant,
    /// Every listener on the port belongs to the adapter's process tree.
    Adapter,
    /// A listener on the port belongs to a process outside the adapter's
    /// process tree. On Linux that includes a listener held by a process
    /// whose sockets cannot be read, such as another user's; on macOS such a
    /// listener is invisible, so a port only it holds reads as
    /// [`PortOwner::Vacant`].
    Foreign,
    /// The socket table cannot be read here, so the owner is not known.
    Unknown,
}

/// The owner of the listeners on `port`, judged against the process tree
/// rooted at `root` (the spawned child; `None` once it has been reaped).
#[cfg(target_os = "linux")]
pub(crate) fn owner(port: u16, root: Option<u32>) -> PortOwner {
    linux::owner(port, root)
}

/// The owner of the listeners on `port`, judged against the process tree
/// rooted at `root` (the spawned child; `None` once it has been reaped).
///
/// Only the listeners this process may inspect are seen: on macOS another
/// user's sockets are invisible, so a port only such a process holds reads as
/// [`PortOwner::Vacant`] and is never connected to.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn owner(port: u16, root: Option<u32>) -> PortOwner {
    if !sys::verified() {
        return PortOwner::Unknown;
    }
    let Some(holders) = sys::listeners(port) else {
        return PortOwner::Unknown;
    };
    if holders.is_empty() {
        return PortOwner::Vacant;
    }
    let tree = match root {
        Some(root) => match sys::parents() {
            Some(parents) => Some(descendants(root, &parents, sys::linked)),
            None => return PortOwner::Unknown,
        },
        None => None,
    };
    judge(&holders, tree.as_deref())
}

/// The owner of the listeners on `port`: never known on this platform.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn owner(_port: u16, _root: Option<u32>) -> PortOwner {
    PortOwner::Unknown
}

/// Judge the pids holding a listener on the port (one entry per listening
/// socket each holds) against the adapter's process tree (`None` once the
/// adapter has been reaped, which leaves it owning nothing).
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn judge(holders: &[u32], tree: Option<&[u32]>) -> PortOwner {
    if holders.is_empty() {
        return PortOwner::Vacant;
    }
    match tree {
        Some(tree) if holders.iter().all(|pid| tree.contains(pid)) => PortOwner::Adapter,
        _ => PortOwner::Foreign,
    }
}

/// A port the kernel stores in network byte order in the low 16 bits of a
/// wider integer (macOS's `insi_lport`, Windows' `dwLocalPort`).
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn port_from_network_order(raw: u32) -> u16 {
    #[allow(clippy::cast_possible_truncation)] // the port is the low 16 bits by definition
    u16::from_be((raw & 0xFFFF) as u16)
}

/// `root` followed by every pid whose chain of `(pid, parent)` links
/// reaches it, following only the links `linked(child, parent)` accepts (a
/// recorded parent pid that may since have been reused is checked there).
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn descendants(
    root: u32,
    parents: &[(u32, u32)],
    mut linked: impl FnMut(u32, u32) -> bool,
) -> Vec<u32> {
    let mut tree = vec![root];
    let mut next = 0;
    while let Some(&pid) = tree.get(next) {
        next += 1;
        for &(child, parent) in parents {
            if parent == pid && !tree.contains(&child) && linked(child, parent) {
                tree.push(child);
            }
        }
    }
    tree
}

/// Whether the platform reader finds a listener this process holds: run once
/// per process, so a reader that misreads the OS's structures is never
/// trusted.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn reads_own_listener(listeners: impl Fn(u16) -> Option<Vec<u32>>) -> bool {
    let Ok(probe) = std::net::TcpListener::bind(("127.0.0.1", 0)) else {
        return false;
    };
    let Ok(addr) = probe.local_addr() else {
        return false;
    };
    listeners(addr.port()).is_some_and(|holders| holders.contains(&std::process::id()))
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashSet;

    use super::PortOwner;
    use super::descendants;

    /// The TCP `LISTEN` state as `/proc/net/tcp*` writes it.
    const LISTEN: &str = "0A";

    pub(super) fn owner(port: u16, root: Option<u32>) -> PortOwner {
        let mut listeners = Vec::new();
        let mut readable = false;
        for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
            if let Ok(text) = std::fs::read_to_string(table) {
                readable = true;
                listeners.extend(listening_inodes(&text, port));
            }
        }
        if !readable {
            return PortOwner::Unknown;
        }
        if listeners.is_empty() {
            return PortOwner::Vacant;
        }
        let Some(root) = root else {
            return PortOwner::Foreign;
        };
        let Some(tree) = process_tree(root) else {
            return PortOwner::Unknown;
        };
        let held = socket_inodes_of(&tree);
        if listeners.iter().all(|inode| held.contains(inode)) {
            PortOwner::Adapter
        } else {
            PortOwner::Foreign
        }
    }

    /// The socket inodes of every listener on `port` in one `/proc/net/tcp*`
    /// table (any local address: a wildcard listener takes loopback connects
    /// too, so it counts as much as a loopback one).
    pub(super) fn listening_inodes(table: &str, port: u16) -> Vec<u64> {
        table
            .lines()
            .skip(1)
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                let local_port = fields.get(1)?.rsplit_once(':')?.1;
                let listening = *fields.get(3)? == LISTEN;
                let on_port = u16::from_str_radix(local_port, 16).ok()? == port;
                (listening && on_port)
                    .then(|| fields.get(9)?.parse().ok())
                    .flatten()
            })
            .collect()
    }

    /// `root` and every process descended from it, or `None` when `/proc`
    /// cannot be listed.
    fn process_tree(root: u32) -> Option<Vec<u32>> {
        let parents: Vec<(u32, u32)> = std::fs::read_dir("/proc")
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
                let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
                Some((pid, parent_pid(&stat)?))
            })
            .collect();
        // The kernel reparents an orphan, so a recorded parent is live.
        Some(descendants(root, &parents, |_, _| true))
    }

    /// The parent pid from a `/proc/PID/stat` line. The command name sits in
    /// parentheses and may itself contain spaces or `)`, so the fields are
    /// read after its last `)`.
    pub(super) fn parent_pid(stat: &str) -> Option<u32> {
        let (_, rest) = stat.rsplit_once(')')?;
        rest.split_whitespace().nth(1)?.parse().ok()
    }

    /// The inodes of every socket the processes in `tree` hold open.
    fn socket_inodes_of(tree: &[u32]) -> HashSet<u64> {
        tree.iter()
            .filter_map(|pid| std::fs::read_dir(format!("/proc/{pid}/fd")).ok())
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|fd| std::fs::read_link(fd.path()).ok())
            .filter_map(|link| socket_inode(link.to_str()?))
            .collect()
    }

    /// The inode of a `/proc/PID/fd` link to a socket (`socket:[INODE]`).
    pub(super) fn socket_inode(link: &str) -> Option<u64> {
        link.strip_prefix("socket:[")?
            .strip_suffix(']')?
            .parse()
            .ok()
    }
}

/// macOS: `libproc` (part of the OS's `libSystem`) lists every process, its
/// parent, and the sockets each holds. The socket record is not in `libc`, so
/// its layout is declared here from `<sys/proc_info.h>` and pinned by
/// compile-time size checks.
#[cfg(target_os = "macos")]
// The structs mirror kernel records field for field so their layout matches;
// only a few of the fields are read.
#[allow(dead_code)]
mod sys {
    use std::ffi::c_int;
    use std::mem::size_of;
    use std::sync::OnceLock;

    /// `proc_listpids` type selecting every process.
    const PROC_ALL_PIDS: u32 = 1;
    /// `proc_pidfdinfo` flavor returning a `socket_fdinfo`.
    const PROC_PIDFDSOCKETINFO: c_int = 3;
    /// `soi_kind` of a TCP socket, whose `soi_proto` is a `tcp_sockinfo`.
    const SOCKINFO_TCP: c_int = 2;
    /// `tcpsi_state` of a listening socket.
    const TSI_S_LISTEN: c_int = 1;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcFileinfo {
        openflags: u32,
        status: u32,
        offset: i64,
        kind: i32,
        guardflags: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SockbufInfo {
        cc: u32,
        hiwat: u32,
        mbcnt: u32,
        mbmax: u32,
        lowat: u32,
        flags: i16,
        timeo: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InV4 {
        tos: u8,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InV6 {
        hlim: u8,
        cksum: c_int,
        ifindex: u16,
        hops: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InSockinfo {
        fport: c_int,
        /// The local port, network byte order in the low 16 bits.
        lport: c_int,
        gencnt: u64,
        flags: u32,
        flow: u32,
        vflag: u8,
        ip_ttl: u8,
        rfu_1: u32,
        faddr: [u32; 4],
        laddr: [u32; 4],
        v4: InV4,
        v6: InV6,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct TcpSockinfo {
        ini: InSockinfo,
        state: c_int,
        timer: [c_int; 4],
        mss: c_int,
        flags: u32,
        rfu_1: u32,
        tp: u64,
    }

    /// `soi_proto`: only the TCP member is read; the byte array is the size
    /// of the largest member (`un_sockinfo`).
    #[repr(C)]
    #[derive(Clone, Copy)]
    union SoiProto {
        tcp: TcpSockinfo,
        largest: [u8; 528],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SocketInfo {
        stat: libc::vinfo_stat,
        so: u64,
        pcb: u64,
        kind_type: c_int,
        protocol: c_int,
        family: c_int,
        options: i16,
        linger: i16,
        state: i16,
        qlen: i16,
        incqlen: i16,
        qlimit: i16,
        timeo: i16,
        error: u16,
        oobmark: u32,
        rcv: SockbufInfo,
        snd: SockbufInfo,
        kind: c_int,
        rfu_1: u32,
        proto: SoiProto,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SocketFdinfo {
        pfi: ProcFileinfo,
        psi: SocketInfo,
    }

    // The `<sys/proc_info.h>` layout this reader depends on.
    const _: () = assert!(size_of::<libc::vinfo_stat>() == 136);
    const _: () = assert!(size_of::<InSockinfo>() == 80);
    const _: () = assert!(size_of::<TcpSockinfo>() == 120);
    const _: () = assert!(std::mem::offset_of!(SocketInfo, proto) == 240);
    const _: () = assert!(size_of::<SocketFdinfo>() == 792);
    /// The bytes a record must fill for its TCP member to be read: the record
    /// up to the end of `tcp_sockinfo`. The union's tail past it is never
    /// read, so a kernel whose larger union members grew still reads.
    const TCP_END: usize = size_of::<ProcFileinfo>()
        + std::mem::offset_of!(SocketInfo, proto)
        + size_of::<TcpSockinfo>();

    /// A `socket_fdinfo` with room past it, so a kernel whose record grew
    /// still writes it rather than refusing a buffer it finds too small.
    #[repr(C)]
    struct SlackRecord {
        info: SocketFdinfo,
        slack: [u8; 256],
    }

    /// Whether this reader finds a listener this process holds (checked once).
    pub(super) fn verified() -> bool {
        static VERIFIED: OnceLock<bool> = OnceLock::new();
        *VERIFIED.get_or_init(|| super::reads_own_listener(listeners))
    }

    /// The pid of each TCP listener on `port` this process may inspect, one
    /// entry per listening descriptor; `None` when processes cannot be listed.
    pub(super) fn listeners(port: u16) -> Option<Vec<u32>> {
        let holders = all_pids()?
            .into_iter()
            .flat_map(|pid| {
                socket_fds(pid)
                    .into_iter()
                    .filter(move |&fd| listen_port(pid, fd) == Some(port))
                    .map(move |_| pid)
            })
            .collect();
        Some(holders)
    }

    /// Whether `child`'s recorded `parent` is its live parent: always, since
    /// launchd adopts an orphan, so a recorded parent pid is never stale.
    pub(super) fn linked(_child: u32, _parent: u32) -> bool {
        true
    }

    /// Every `(pid, parent)` pair; `None` when processes cannot be listed.
    pub(super) fn parents() -> Option<Vec<(u32, u32)>> {
        Some(
            all_pids()?
                .into_iter()
                .filter_map(|pid| Some((pid, parent_of(pid)?)))
                .collect(),
        )
    }

    fn all_pids() -> Option<Vec<u32>> {
        // SAFETY: a null buffer asks only for the byte count needed.
        let needed = unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
        let slots = usize::try_from(needed).ok()? / size_of::<c_int>() + 64;
        let mut pids: Vec<c_int> = vec![0; slots];
        let bytes = c_int::try_from(slots * size_of::<c_int>()).ok()?;
        // SAFETY: `pids` holds `bytes` writable bytes of `c_int`s.
        let written =
            unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, pids.as_mut_ptr().cast(), bytes) };
        let count = usize::try_from(written).ok()? / size_of::<c_int>();
        pids.truncate(count);
        Some(
            pids.into_iter()
                .filter_map(|pid| u32::try_from(pid).ok())
                .filter(|&pid| pid != 0)
                .collect(),
        )
    }

    fn parent_of(pid: u32) -> Option<u32> {
        // SAFETY: `proc_bsdinfo` is plain integers and byte arrays, so all
        // zeroes is a valid value.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = c_int::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
        // SAFETY: `info` is a writable `proc_bsdinfo` of `size` bytes.
        let written = unsafe {
            libc::proc_pidinfo(
                c_int::try_from(pid).ok()?,
                libc::PROC_PIDTBSDINFO,
                0,
                (&raw mut info).cast(),
                size,
            )
        };
        (written == size).then_some(info.pbi_ppid)
    }

    /// The socket descriptors `pid` holds (empty when it cannot be read).
    fn socket_fds(pid: u32) -> Vec<c_int> {
        let Ok(pid) = c_int::try_from(pid) else {
            return Vec::new();
        };
        // SAFETY: a null buffer asks only for the byte count needed.
        let needed =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        let Ok(needed) = usize::try_from(needed) else {
            return Vec::new();
        };
        let empty = libc::proc_fdinfo {
            proc_fd: 0,
            proc_fdtype: 0,
        };
        let mut fds = vec![empty; needed / size_of::<libc::proc_fdinfo>() + 16];
        let Ok(bytes) = c_int::try_from(fds.len() * size_of::<libc::proc_fdinfo>()) else {
            return Vec::new();
        };
        // SAFETY: `fds` holds `bytes` writable bytes of `proc_fdinfo`s.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr().cast(),
                bytes,
            )
        };
        let count = usize::try_from(written).unwrap_or(0) / size_of::<libc::proc_fdinfo>();
        fds.truncate(count);
        fds.into_iter()
            .filter(|fd| {
                u32::try_from(libc::PROX_FDTYPE_SOCKET).is_ok_and(|kind| fd.proc_fdtype == kind)
            })
            .map(|fd| fd.proc_fd)
            .collect()
    }

    /// The port descriptor `fd` of `pid` listens on, when it is a TCP
    /// listener.
    fn listen_port(pid: u32, fd: c_int) -> Option<u16> {
        // SAFETY: the record is plain integers, byte arrays and a union of
        // them, so all zeroes is a valid value.
        let mut record: SlackRecord = unsafe { std::mem::zeroed() };
        let size = c_int::try_from(size_of::<SlackRecord>()).ok()?;
        // SAFETY: `record` is `size` writable bytes, at least a whole
        // `socket_fdinfo`.
        let written = unsafe {
            libc::proc_pidfdinfo(
                c_int::try_from(pid).ok()?,
                fd,
                PROC_PIDFDSOCKETINFO,
                (&raw mut record).cast(),
                size,
            )
        };
        let info = record.info;
        if usize::try_from(written).map_or(true, |written| written < TCP_END)
            || info.psi.kind != SOCKINFO_TCP
        {
            return None;
        }
        // SAFETY: `soi_kind == SOCKINFO_TCP` says the kernel wrote the TCP
        // member, and every bit pattern is a valid `tcp_sockinfo`.
        let tcp = unsafe { info.psi.proto.tcp };
        (tcp.state == TSI_S_LISTEN)
            .then(|| {
                u32::try_from(tcp.ini.lport)
                    .ok()
                    .map(super::port_from_network_order)
            })
            .flatten()
    }
}

/// Windows: `GetExtendedTcpTable` names the owning pid of every TCP listener,
/// and a Toolhelp snapshot gives each process's parent.
#[cfg(target_os = "windows")]
mod sys {
    use std::mem::size_of;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Foundation::NO_ERROR;
    use windows_sys::Win32::NetworkManagement::IpHelper::GetExtendedTcpTable;
    use windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCP_STATE_LISTEN;
    use windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCP6ROW_OWNER_PID;
    use windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCPROW_OWNER_PID;
    use windows_sys::Win32::NetworkManagement::IpHelper::TCP_TABLE_OWNER_PID_LISTENER;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::CreateToolhelp32Snapshot;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::PROCESSENTRY32W;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::Process32FirstW;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::Process32NextW;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::TH32CS_SNAPPROCESS;
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    use windows_sys::Win32::System::Threading::OpenProcess;
    use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;

    /// `AF_INET` / `AF_INET6` as `GetExtendedTcpTable` takes them.
    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    /// How many times a table that grew between sizing and reading is re-read.
    const TABLE_ATTEMPTS: usize = 4;

    /// Whether this reader finds a listener this process holds (checked once).
    pub(super) fn verified() -> bool {
        static VERIFIED: OnceLock<bool> = OnceLock::new();
        *VERIFIED.get_or_init(|| super::reads_own_listener(listeners))
    }

    /// The owning pid of each TCP listener (IPv4 and IPv6) on `port`; `None`
    /// when either table cannot be read.
    pub(super) fn listeners(port: u16) -> Option<Vec<u32>> {
        let listening = u32::try_from(MIB_TCP_STATE_LISTEN).ok()?;
        let v4 = table::<MIB_TCPROW_OWNER_PID>(AF_INET)?
            .into_iter()
            .filter(|row| {
                row.dwState == listening && super::port_from_network_order(row.dwLocalPort) == port
            })
            .map(|row| row.dwOwningPid);
        let v6 = table::<MIB_TCP6ROW_OWNER_PID>(AF_INET6)?
            .into_iter()
            .filter(|row| {
                row.dwState == listening && super::port_from_network_order(row.dwLocalPort) == port
            })
            .map(|row| row.dwOwningPid);
        Some(v4.chain(v6).collect())
    }

    /// The listener rows of the `family` table: a `u32` count followed by
    /// that many `Row`s.
    fn table<Row: Copy>(family: u32) -> Option<Vec<Row>> {
        let mut size: u32 = 0;
        for _ in 0..TABLE_ATTEMPTS {
            // `u32` words keep the count and every row (all `u32`-aligned)
            // aligned.
            let mut buffer: Vec<u32> = vec![0; usize::try_from(size).ok()? / size_of::<u32>() + 1];
            // SAFETY: `buffer` holds at least `size` writable bytes; the call
            // writes at most `size` bytes, or updates `size` and writes none.
            let status = unsafe {
                GetExtendedTcpTable(
                    buffer.as_mut_ptr().cast(),
                    &raw mut size,
                    0,
                    family,
                    TCP_TABLE_OWNER_PID_LISTENER,
                    0,
                )
            };
            if status == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            if status != NO_ERROR {
                return None;
            }
            let count = usize::try_from(*buffer.first()?).ok()?;
            let rows_bytes = count.checked_mul(size_of::<Row>())?;
            if size_of::<u32>() + rows_bytes > buffer.len() * size_of::<u32>() {
                return None;
            }
            let rows = buffer.as_ptr().wrapping_add(1).cast::<Row>();
            // SAFETY: the bounds check above keeps every row inside `buffer`,
            // which the call filled with `count` rows after the count.
            return Some(
                (0..count)
                    .map(|i| unsafe { rows.add(i).read_unaligned() })
                    .collect(),
            );
        }
        None
    }

    /// Whether `child`'s recorded `parent` pid still names its parent.
    /// Windows never reparents an orphan, so the pid of a parent that exited
    /// can since have been reused by an unrelated process; only a process
    /// created no later than the child can be its parent. A link whose
    /// creation times cannot be read is not followed.
    pub(super) fn linked(child: u32, parent: u32) -> bool {
        match (created(parent), created(child)) {
            (Some(parent), Some(child)) => parent <= child,
            _ => false,
        }
    }

    /// When process `pid` was created, in 100ns ticks; `None` when it cannot
    /// be opened (exited, or not this user's).
    fn created(pid: u32) -> Option<u64> {
        // SAFETY: no pointers; a null handle is checked below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return None;
        }
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: `process` is open with query rights, and each pointer is a
        // live `FILETIME`.
        let read = unsafe {
            GetProcessTimes(
                process,
                &raw mut creation,
                &raw mut exit,
                &raw mut kernel,
                &raw mut user,
            )
        } != 0;
        // SAFETY: `process` is open and closed only here.
        unsafe { CloseHandle(process) };
        read.then(|| u64::from(creation.dwHighDateTime) << 32 | u64::from(creation.dwLowDateTime))
    }

    /// Every `(pid, parent)` pair in a process snapshot; `None` when no
    /// snapshot can be taken.
    pub(super) fn parents() -> Option<Vec<(u32, u32)>> {
        // SAFETY: a process snapshot takes no pointers; the handle is closed
        // below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return None;
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(0),
            ..PROCESSENTRY32W::default()
        };
        let mut pairs = Vec::new();
        // SAFETY: `snapshot` is open and `entry` is a sized `PROCESSENTRY32W`.
        let mut more = unsafe { Process32FirstW(snapshot, &raw mut entry) } != 0;
        while more {
            pairs.push((entry.th32ProcessID, entry.th32ParentProcessID));
            // SAFETY: as for `Process32FirstW`.
            more = unsafe { Process32NextW(snapshot, &raw mut entry) } != 0;
        }
        // SAFETY: `snapshot` is open and closed only here.
        unsafe { CloseHandle(snapshot) };
        Some(pairs)
    }
}

#[cfg(test)]
mod tests {
    use super::PortOwner;
    use super::descendants;
    use super::judge;
    use super::port_from_network_order;

    #[test]
    fn descendants_follow_every_generation_and_nothing_else() {
        // 10 → 11 → 12, 10 → 13; 20 → 21 is another tree.
        let parents = [(11, 10), (12, 11), (13, 10), (21, 20), (10, 1)];
        assert_eq!(descendants(10, &parents, |_, _| true), [10, 11, 13, 12]);
        assert_eq!(descendants(12, &parents, |_, _| true), [12]);
    }

    #[test]
    fn descendants_stop_at_a_parent_cycle() {
        // A reused pid can make two processes each other's parent.
        let parents = [(11, 10), (10, 11)];
        assert_eq!(descendants(10, &parents, |_, _| true), [10, 11]);
    }

    #[test]
    fn descendants_skip_a_link_the_check_refuses_and_everything_below_it() {
        // 12 records a parent pid 10 that the adapter reused after 12's real
        // parent exited: neither 12 nor its own child 14 is the adapter's.
        let parents = [(11, 10), (12, 10), (14, 12)];
        let tree = descendants(10, &parents, |child, _| child != 12);
        assert_eq!(tree, [10, 11]);
        assert_eq!(judge(&[12], Some(&tree)), PortOwner::Foreign);
    }

    #[test]
    fn judge_owns_the_port_only_when_every_holder_is_in_the_tree() {
        let tree = [10, 11, 12];
        assert_eq!(judge(&[], Some(&tree)), PortOwner::Vacant);
        assert_eq!(judge(&[], None), PortOwner::Vacant);
        assert_eq!(judge(&[11], Some(&tree)), PortOwner::Adapter);
        // The v4 and v6 listener of one adapter, held by two of its processes.
        assert_eq!(judge(&[11, 12], Some(&tree)), PortOwner::Adapter);
        // One listener outside the tree is enough to make the port foreign.
        assert_eq!(judge(&[11, 99], Some(&tree)), PortOwner::Foreign);
        assert_eq!(judge(&[99], Some(&tree)), PortOwner::Foreign);
        // A reaped adapter owns nothing.
        assert_eq!(judge(&[11], None), PortOwner::Foreign);
    }

    #[test]
    fn port_from_network_order_reads_the_low_16_bits_big_endian() {
        // 8080 = 0x1F90, stored as the bytes 1F 90 in memory.
        let stored = u32::from(u16::from_ne_bytes([0x1F, 0x90]));
        assert_eq!(port_from_network_order(stored), 8080);
        // High bits (garbage in Windows' `dwLocalPort`) are ignored.
        assert_eq!(port_from_network_order(stored | 0xABCD_0000), 8080);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::linux::listening_inodes;
    use super::linux::parent_pid;
    use super::linux::socket_inode;

    #[test]
    fn listening_inodes_reads_only_listeners_on_the_port() {
        // 0x1F90 = 8080. Row 1 listens on loopback, row 2 is an established
        // connection on the same port, row 3 listens on another port, row 4
        // is a v6 wildcard listener.
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0 100 0 0 10 0
   1: 0100007F:1F90 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 222 1 0 20 4 30 10 -1
   2: 0100007F:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 333 1 0 100 0 0 10 0
   3: 00000000000000000000000000000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 444 1 0 100 0 0 10 0
";
        assert_eq!(listening_inodes(table, 8080), [111, 444]);
        assert_eq!(listening_inodes(table, 8081), [333]);
        assert!(listening_inodes(table, 9).is_empty());
    }

    #[test]
    fn parent_pid_reads_past_a_command_name_holding_parens_and_spaces() {
        assert_eq!(parent_pid("42 (a) b) (c) S 7 42 42 0 -1"), Some(7));
        assert_eq!(parent_pid("42 (cat) S 1 42"), Some(1));
        assert_eq!(parent_pid("garbage"), None);
    }

    #[test]
    fn socket_inode_reads_only_socket_links() {
        assert_eq!(socket_inode("socket:[12345]"), Some(12345));
        assert_eq!(socket_inode("pipe:[12345]"), None);
        assert_eq!(socket_inode("/dev/null"), None);
    }
}

/// The live checks against the real socket table, on every platform that has
/// a reader.
#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod live_tests {
    use super::PortOwner;
    use super::owner;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// A child that lives a few seconds and holds no socket.
    fn idle_child() -> std::io::Result<std::process::Child> {
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("ping");
            command.args(["-n", "6", "127.0.0.1"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = std::process::Command::new("sleep");
            command.arg("5");
            command
        };
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn the_reader_finds_a_listener_this_process_holds() {
        // Without this, `owner` is `Unknown` everywhere and the other live
        // tests below would fail on it.
        assert!(super::sys::verified());
    }

    #[test]
    fn a_listener_is_the_adapters_only_inside_its_process_tree() -> TestResult {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        // This process holds the listener: its own tree owns it.
        assert_eq!(owner(port, Some(std::process::id())), PortOwner::Adapter);
        // A child of this process is a tree that does not hold it.
        let mut child = idle_child()?;
        let foreign = owner(port, Some(child.id()));
        child.kill()?;
        child.wait()?;
        assert_eq!(foreign, PortOwner::Foreign);
        // A reaped child owns nothing.
        assert_eq!(owner(port, None), PortOwner::Foreign);
        drop(listener);
        Ok(())
    }

    #[test]
    fn a_port_nobody_listens_on_is_vacant() -> TestResult {
        // Bound but not listening: no LISTEN row, and no concurrent test can
        // start listening on the port while the socket holds it.
        let socket = tokio::net::TcpSocket::new_v4()?;
        socket.bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))?;
        let port = socket.local_addr()?.port();
        assert_eq!(owner(port, Some(std::process::id())), PortOwner::Vacant);
        Ok(())
    }
}
