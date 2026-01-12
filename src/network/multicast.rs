//! Multicast socket management for RTP streams.

#![allow(dead_code)]

use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashSet;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use thiserror::Error;
use tokio::net::UdpSocket as TokioUdpSocket;

#[derive(Error, Debug)]
pub enum MulticastError {
    #[error("Socket error: {0}")]
    Socket(#[from] io::Error),

    #[error("Address {0} is not a valid multicast address")]
    NotMulticast(Ipv4Addr),

    #[error("Already joined group {0}")]
    AlreadyJoined(Ipv4Addr),

    #[error("Not a member of group {0}")]
    NotMember(Ipv4Addr),
}

/// A multicast-capable UDP socket
pub struct MulticastSocket {
    socket: TokioUdpSocket,
    port: u16,
    joined_groups: HashSet<Ipv4Addr>,
    interface: Ipv4Addr,
    /// The multicast group this socket is bound to (for filtering)
    bound_group: Option<Ipv4Addr>,
}

impl MulticastSocket {
    /// Create a new multicast socket bound to the specified port
    pub async fn new(port: u16) -> Result<Self, MulticastError> {
        Self::with_interface(port, Ipv4Addr::UNSPECIFIED).await
    }

    /// Create a new multicast socket bound to a specific interface
    #[allow(clippy::unused_async)] // Async for API consistency with future enhancements
    pub async fn with_interface(port: u16, interface: Ipv4Addr) -> Result<Self, MulticastError> {
        // Create socket with socket2 for fine-grained control
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

        // Allow multiple processes to bind to same port
        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;

        // Set non-blocking before converting
        socket.set_nonblocking(true)?;

        // Bind to the port on all interfaces
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
        socket.bind(&addr.into())?;

        // Convert to std socket, then to tokio
        let std_socket: UdpSocket = socket.into();
        let tokio_socket = TokioUdpSocket::from_std(std_socket)?;

        Ok(Self {
            socket: tokio_socket,
            port,
            joined_groups: HashSet::new(),
            interface,
            bound_group: None,
        })
    }

    /// Create a new multicast socket bound to a specific multicast group address.
    /// This ensures the socket only receives packets destined for this specific group,
    /// even when multiple sockets share the same port with SO_REUSEPORT.
    #[allow(clippy::unused_async)]
    pub async fn bound_to_group(group: Ipv4Addr, port: u16, interface: Ipv4Addr) -> Result<Self, MulticastError> {
        if !group.is_multicast() {
            return Err(MulticastError::NotMulticast(group));
        }

        // Create socket with socket2 for fine-grained control
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

        // Allow multiple processes to bind to same port
        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;

        // Set non-blocking before converting
        socket.set_nonblocking(true)?;

        // Bind to the multicast group address directly.
        // On Linux, this ensures the socket only receives packets destined for this group.
        let addr = SocketAddrV4::new(group, port);
        socket.bind(&addr.into())?;

        // Convert to std socket, then to tokio
        let std_socket: UdpSocket = socket.into();
        let tokio_socket = TokioUdpSocket::from_std(std_socket)?;

        // Join the multicast group
        tokio_socket.join_multicast_v4(group, interface)?;

        let mut joined_groups = HashSet::new();
        joined_groups.insert(group);

        Ok(Self {
            socket: tokio_socket,
            port,
            joined_groups,
            interface,
            bound_group: Some(group),
        })
    }

    /// Get the multicast group this socket is bound to (if any)
    pub fn bound_group(&self) -> Option<Ipv4Addr> {
        self.bound_group
    }

    /// Join a multicast group
    pub fn join(&mut self, group: Ipv4Addr) -> Result<(), MulticastError> {
        if !group.is_multicast() {
            return Err(MulticastError::NotMulticast(group));
        }

        if self.joined_groups.contains(&group) {
            return Err(MulticastError::AlreadyJoined(group));
        }

        self.socket.join_multicast_v4(group, self.interface)?;
        self.joined_groups.insert(group);

        Ok(())
    }

    /// Leave a multicast group
    pub fn leave(&mut self, group: Ipv4Addr) -> Result<(), MulticastError> {
        if !self.joined_groups.contains(&group) {
            return Err(MulticastError::NotMember(group));
        }

        self.socket.leave_multicast_v4(group, self.interface)?;
        self.joined_groups.remove(&group);

        Ok(())
    }

    /// Leave all multicast groups
    pub fn leave_all(&mut self) -> Result<(), MulticastError> {
        let groups: Vec<Ipv4Addr> = self.joined_groups.iter().copied().collect();
        for group in groups {
            self.socket.leave_multicast_v4(group, self.interface)?;
        }
        self.joined_groups.clear();
        Ok(())
    }

    /// Receive a packet
    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), io::Error> {
        self.socket.recv_from(buf).await
    }

    /// Send a packet to a multicast address
    pub async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> Result<usize, io::Error> {
        self.socket.send_to(buf, addr).await
    }

    /// Set the multicast TTL
    pub fn set_multicast_ttl(&self, ttl: u32) -> Result<(), io::Error> {
        self.socket.set_multicast_ttl_v4(ttl)
    }

    /// Disable multicast loopback (don't receive our own packets)
    pub fn set_multicast_loop(&self, enable: bool) -> Result<(), io::Error> {
        self.socket.set_multicast_loop_v4(enable)
    }

    /// Get the port this socket is bound to
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Get the list of joined groups
    pub fn joined_groups(&self) -> &HashSet<Ipv4Addr> {
        &self.joined_groups
    }

    /// Check if a group is joined
    pub fn is_member(&self, group: Ipv4Addr) -> bool {
        self.joined_groups.contains(&group)
    }
}

/// A pool of multicast sockets, one per port
pub struct MulticastSocketPool {
    sockets: std::collections::HashMap<u16, MulticastSocket>,
}

impl MulticastSocketPool {
    pub fn new() -> Self {
        Self {
            sockets: std::collections::HashMap::new(),
        }
    }

    /// Get or create a socket for the given port
    pub async fn get_or_create(&mut self, port: u16) -> Result<&mut MulticastSocket, MulticastError> {
        // Use entry API to avoid unwrap after contains_key check
        use std::collections::hash_map::Entry;
        match self.sockets.entry(port) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let socket = MulticastSocket::new(port).await?;
                Ok(entry.insert(socket))
            }
        }
    }

    /// Join a multicast group on the appropriate socket
    pub async fn join(&mut self, group: Ipv4Addr, port: u16) -> Result<(), MulticastError> {
        let socket = self.get_or_create(port).await?;
        // Ignore AlreadyJoined errors
        match socket.join(group) {
            Ok(()) => Ok(()),
            Err(MulticastError::AlreadyJoined(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Leave a multicast group
    #[allow(clippy::unused_async)] // Async for API consistency
    pub async fn leave(&mut self, group: Ipv4Addr, port: u16) -> Result<(), MulticastError> {
        if let Some(socket) = self.sockets.get_mut(&port) {
            socket.leave(group)?;
        }
        Ok(())
    }

    /// Get a reference to a socket by port
    pub fn get(&self, port: u16) -> Option<&MulticastSocket> {
        self.sockets.get(&port)
    }

    /// Get all sockets
    pub fn sockets(&self) -> impl Iterator<Item = &MulticastSocket> {
        self.sockets.values()
    }

    /// Get mutable reference to all sockets
    pub fn sockets_mut(&mut self) -> impl Iterator<Item = &mut MulticastSocket> {
        self.sockets.values_mut()
    }
}

impl Default for MulticastSocketPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a transmit-only multicast socket
pub async fn create_transmit_socket(ttl: u8) -> Result<TokioUdpSocket, io::Error> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

    // Bind to any available port
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    socket.bind(&addr.into())?;

    socket.set_nonblocking(true)?;

    let std_socket: UdpSocket = socket.into();
    let tokio_socket = TokioUdpSocket::from_std(std_socket)?;

    tokio_socket.set_multicast_ttl_v4(ttl as u32)?;
    // Enable loopback so we can monitor our own transmissions on the same machine
    tokio_socket.set_multicast_loop_v4(true)?;

    Ok(tokio_socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_socket() {
        let socket = MulticastSocket::new(0).await;
        assert!(socket.is_ok());
    }

    #[tokio::test]
    async fn test_join_leave() {
        let mut socket = MulticastSocket::new(0).await.unwrap();
        let group = Ipv4Addr::new(224, 0, 1, 1);

        assert!(socket.join(group).is_ok());
        assert!(socket.is_member(group));

        assert!(socket.leave(group).is_ok());
        assert!(!socket.is_member(group));
    }

    #[tokio::test]
    async fn test_invalid_multicast() {
        let mut socket = MulticastSocket::new(0).await.unwrap();
        let result = socket.join(Ipv4Addr::new(192, 168, 1, 1));
        assert!(matches!(result, Err(MulticastError::NotMulticast(_))));
    }

    #[tokio::test]
    async fn test_socket_pool() {
        let mut pool = MulticastSocketPool::new();
        let group = Ipv4Addr::new(224, 0, 1, 1);

        assert!(pool.join(group, 5004).await.is_ok());
        assert!(pool.get(5004).is_some());
        assert!(pool.get(5004).unwrap().is_member(group));
    }
}
