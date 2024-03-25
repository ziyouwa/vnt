use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;
use std::{io, thread};

use crate::channel::punch::NatType;
use std::net::UdpSocket;
use stun_format::Attr;

pub fn stun_test_nat(stun_servers: Vec<String>) -> io::Result<(NatType, Vec<Ipv4Addr>, u16)> {
    let mut h = Vec::new();
    for x in stun_servers {
        let handle = thread::spawn(move || test_nat(x));
        h.push(handle);
    }
    let mut nat_type = NatType::Cone;
    let mut port_range = 0;
    let mut hash_set = HashSet::new();
    for x in h {
        if let Ok(Ok((nat_type_t, ip_list_t, port_range_t))) = x.join() {
            // if let Ok((nat_type_t, ip_list_t, port_range_t)) = rs {
                if nat_type_t == NatType::Symmetric {
                    nat_type = NatType::Symmetric;
                }
                for y in ip_list_t {
                    hash_set.insert(y);
                }
                if port_range < port_range_t {
                    port_range = port_range_t;
                }
            // }
        }
    }
    Ok((nat_type, hash_set.into_iter().collect(), port_range))
}

fn test_nat(stun_server: String) -> io::Result<(NatType, Vec<Ipv4Addr>, u16)> {
    let udp = UdpSocket::bind("0.0.0.0:0")?;
    udp.set_read_timeout(Some(Duration::from_millis(300)))?;
    udp.connect(stun_server)?;
    let mut port_range = 0;
    let mut hash_set = HashSet::new();
    let mut nat_type = NatType::Cone;
    if let Ok((mapped_addr1, changed_addr1)) = test_nat_(&udp, true, true) {
        match mapped_addr1.ip() {
            IpAddr::V4(ip) => {
                hash_set.insert(ip);
            }
            IpAddr::V6(_) => {}
        }
        if udp.connect(changed_addr1).is_ok() {
            if let Ok((mapped_addr2, _)) = test_nat_(&udp, false, false) {
                match mapped_addr2.ip() {
                    IpAddr::V4(ip) => {
                        hash_set.insert(ip);
                        if mapped_addr1 != mapped_addr2 {
                            nat_type = NatType::Symmetric;
                        }
                    }
                    IpAddr::V6(_) => {}
                }
                port_range = mapped_addr2.port().abs_diff(mapped_addr1.port());
            }
        }
    }
    Ok((nat_type, hash_set.into_iter().collect(), port_range))
}

fn test_nat_(
    udp: &UdpSocket,
    change_ip: bool,
    change_port: bool,
) -> io::Result<(SocketAddr, SocketAddr)> {
    for _ in 0..2 {
        let mut buf = [0u8; 28];
        let mut msg = stun_format::MsgBuilder::from(buf.as_mut_slice());
        msg.typ(stun_format::MsgType::BindingRequest).unwrap();
        msg.tid(1).unwrap();
        msg.add_attr(Attr::ChangeRequest {
            change_ip,
            change_port,
        })
        .unwrap();
        udp.send(msg.as_bytes())?;
        let mut buf = [0; 10240];
        let (len, _addr) = match udp.recv_from(&mut buf) {
            Ok(rs) => rs,
            Err(_) => {
                continue;
            }
        };
        let msg = stun_format::Msg::from(&buf[..len]);
        let mut mapped_addr = None;
        let mut changed_addr = None;
        for x in msg.attrs_iter() {
            match x {
                Attr::MappedAddress(addr) => {
                    if mapped_addr.is_none() {
                        let _ = mapped_addr.insert(stun_addr(addr));
                    }
                }
                Attr::ChangedAddress(addr) => {
                    if changed_addr.is_none() {
                        let _ = changed_addr.insert(stun_addr(addr));
                    }
                }
                Attr::XorMappedAddress(addr) => {
                    if mapped_addr.is_none() {
                        let _ = mapped_addr.insert(stun_addr(addr));
                    }
                }
                _ => {}
            }
            if let (Some(changed_addr),Some(mapped_addr)) = (changed_addr, mapped_addr) {
                return Ok((mapped_addr, changed_addr));
            }
        }
        if let Some(addr) = mapped_addr {
            return Ok((addr, changed_addr.unwrap_or(addr)));
        }
    }
    Err(io::Error::new(io::ErrorKind::Other, "stun response err"))
}

fn stun_addr(addr: stun_format::SocketAddr) -> SocketAddr {
    match addr {
        stun_format::SocketAddr::V4(ip, port) => {
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(ip), port))
        }
        stun_format::SocketAddr::V6(ip, port) => {
            SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(ip), port, 0, 0))
        }
    }
}
