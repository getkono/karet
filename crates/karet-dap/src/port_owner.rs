//! Who listens on a spawn-then-TCP adapter's port: the adapter's own process
//! tree, or some other process that took the port between the client choosing
//! it and the adapter binding it.
//!
//! Read from the kernel's socket table where the platform exposes one without
//! a new dependency: Linux's `/proc`. Elsewhere the owner is
//! [`PortOwner::Unknown`] and the launch falls back to watching the adapter's
//! exit.

/// What the socket table says about the listeners on a loopback port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortOwner {
    /// Nothing listens on the port yet.
    Vacant,
    /// Every listener on the port belongs to the adapter's process tree.
    Adapter,
    /// A listener on the port belongs to a process outside the adapter's
    /// process tree (or to one whose sockets cannot be read, such as another
    /// user's).
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

/// The owner of the listeners on `port`: never known on this platform.
#[cfg(not(target_os = "linux"))]
pub(crate) fn owner(_port: u16, _root: Option<u32>) -> PortOwner {
    PortOwner::Unknown
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashSet;

    use super::PortOwner;

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
        Some(descendants(root, &parents))
    }

    /// The parent pid from a `/proc/PID/stat` line. The command name sits in
    /// parentheses and may itself contain spaces or `)`, so the fields are
    /// read after its last `)`.
    pub(super) fn parent_pid(stat: &str) -> Option<u32> {
        let (_, rest) = stat.rsplit_once(')')?;
        rest.split_whitespace().nth(1)?.parse().ok()
    }

    /// `root` followed by every pid whose chain of `(pid, parent)` links
    /// reaches it.
    pub(super) fn descendants(root: u32, parents: &[(u32, u32)]) -> Vec<u32> {
        let mut tree = vec![root];
        let mut next = 0;
        while let Some(&pid) = tree.get(next) {
            next += 1;
            for &(child, parent) in parents {
                if parent == pid && !tree.contains(&child) {
                    tree.push(child);
                }
            }
        }
        tree
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::PortOwner;
    use super::linux::descendants;
    use super::linux::listening_inodes;
    use super::linux::parent_pid;
    use super::linux::socket_inode;
    use super::owner;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

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
    fn descendants_follow_every_generation_and_nothing_else() {
        // 10 → 11 → 12, 10 → 13; 20 → 21 is another tree.
        let parents = [(11, 10), (12, 11), (13, 10), (21, 20), (10, 1)];
        assert_eq!(descendants(10, &parents), [10, 11, 13, 12]);
        assert_eq!(descendants(12, &parents), [12]);
    }

    #[test]
    fn socket_inode_reads_only_socket_links() {
        assert_eq!(socket_inode("socket:[12345]"), Some(12345));
        assert_eq!(socket_inode("pipe:[12345]"), None);
        assert_eq!(socket_inode("/dev/null"), None);
    }

    #[test]
    fn a_listener_is_the_adapters_only_inside_its_process_tree() -> TestResult {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        // This process holds the listener: its own tree owns it.
        assert_eq!(owner(port, Some(std::process::id())), PortOwner::Adapter);
        // A child of this process is a tree that does not hold it.
        let mut child = std::process::Command::new("sleep").arg("5").spawn()?;
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
