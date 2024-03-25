use std::net::Ipv4Addr;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::time::Duration;
use std::{io, thread};

use crossbeam_utils::atomic::AtomicCell;
use parking_lot::Mutex;
use protobuf::Message;
use rand::prelude::SliceRandom;

use crate::channel::context::Context;
use crate::channel::punch::{NatInfo, NatType, Punch};
use crate::cipher::Cipher;
use crate::handle::{CurrentDeviceInfo, PeerDeviceInfo};
use crate::nat::NatTest;
use crate::proto::message::{PunchInfo, PunchNatType};
use crate::protocol::body::ENCRYPTION_RESERVED;
use crate::protocol::{control_packet, other_turn_packet, NetPacket, Protocol, Version, MAX_TTL};
use crate::util::Scheduler;

#[derive(Clone)]
pub struct PunchSender {
    sender_self: SyncSender<(Ipv4Addr, NatInfo)>,
    sender_peer: SyncSender<(Ipv4Addr, NatInfo)>,
    sender_cone_self: SyncSender<(Ipv4Addr, NatInfo)>,
    sender_cone_peer: SyncSender<(Ipv4Addr, NatInfo)>,
}
impl PunchSender {
    pub fn send(&self, src_peer: bool, ip: Ipv4Addr, info: NatInfo) -> bool {
        log::info!(
            "发送打洞协商消息,是否对端发起:{},ip:{},info:{:?}",
            src_peer,
            ip,
            info
        );
        let sender = match info.nat_type {
            NatType::Symmetric => {
                if src_peer {
                    &self.sender_peer
                } else {
                    &self.sender_self
                }
            }
            NatType::Cone => {
                if src_peer {
                    &self.sender_cone_peer
                } else {
                    &self.sender_cone_self
                }
            }
        };
        sender.try_send((ip, info)).is_ok()
    }
}
pub struct PunchReceiver {
    receiver_peer: Receiver<(Ipv4Addr, NatInfo)>,
    receiver_self: Receiver<(Ipv4Addr, NatInfo)>,
    receiver_cone_peer: Receiver<(Ipv4Addr, NatInfo)>,
    receiver_cone_self: Receiver<(Ipv4Addr, NatInfo)>,
}
pub fn punch_channel() -> (PunchSender, PunchReceiver) {
    let (sender_self, receiver_self) = sync_channel(1);
    let (sender_peer, receiver_peer) = sync_channel(1);
    let (sender_cone_peer, receiver_cone_peer) = sync_channel(1);
    let (sender_cone_self, receiver_cone_self) = sync_channel(1);
    (
        PunchSender {
            sender_self,
            sender_peer,
            sender_cone_peer,
            sender_cone_self,
        },
        PunchReceiver {
            receiver_peer,
            receiver_self,
            receiver_cone_peer,
            receiver_cone_self,
        },
    )
}

pub fn punch(
    scheduler: &Scheduler,
    context: Context,
    nat_test: NatTest,
    device_list: Arc<Mutex<(u16, Vec<PeerDeviceInfo>)>>,
    current_device: Arc<AtomicCell<CurrentDeviceInfo>>,
    client_cipher: Cipher,
    receiver: PunchReceiver,
    punch: Punch,
) {
    punch_request(
        scheduler,
        context,
        nat_test,
        device_list,
        current_device.clone(),
        client_cipher.clone(),
        0,
    );
    let f = |receiver: Receiver<(Ipv4Addr, NatInfo)>| {
        let punch = punch.clone();
        let current_device = current_device.clone();
        let client_cipher = client_cipher.clone();
        thread::Builder::new()
            .name("punch".into())
            .spawn(move || {
                punch_start(receiver, punch, current_device, client_cipher);
            })
            .expect("punch");
    };
    f(receiver.receiver_peer);
    f(receiver.receiver_self);
    f(receiver.receiver_cone_peer);
    f(receiver.receiver_cone_self);
}

/// 接收打洞消息，配合对端打洞
fn punch_start(
    receiver: Receiver<(Ipv4Addr, NatInfo)>,
    mut punch: Punch,
    current_device: Arc<AtomicCell<CurrentDeviceInfo>>,
    client_cipher: Cipher,
) {
    while let Ok((peer_ip, nat_info)) = receiver.recv() {
        let mut packet = NetPacket::new_encrypt([0u8; 12 + ENCRYPTION_RESERVED]).unwrap();
        packet.set_version(Version::V1);
        packet.first_set_ttl(1);
        packet.set_protocol(Protocol::Control);
        packet.set_transport_protocol(control_packet::Protocol::PunchRequest.into());
        packet.set_source(current_device.load().virtual_ip());
        packet.set_destination(peer_ip);
        log::info!("发起打洞，目标:{:?},{:?}", peer_ip, nat_info);
        if let Err(e) = client_cipher.encrypt_ipv4(&mut packet) {
            log::error!("{:?}", e);
            continue;
        }
        if let Err(e) = punch.punch(packet.buffer(), peer_ip, nat_info) {
            log::warn!("{:?}", e)
        }
    }
}

/// 定时发起打洞请求
fn punch_request(
    scheduler: &Scheduler,
    context: Context,
    nat_test: NatTest,
    device_list: Arc<Mutex<(u16, Vec<PeerDeviceInfo>)>>,
    current_device: Arc<AtomicCell<CurrentDeviceInfo>>,
    client_cipher: Cipher,
    count: usize,
) {
    let curr = current_device.load();
    let secs = if curr.status.online() {
        if let Err(e) = punch0(&context, &nat_test, &device_list, curr, &client_cipher) {
            log::warn!("{:?}", e)
        }
        let sleep_time = [3, 5, 7, 11, 13, 17, 19, 23, 29];
        Duration::from_secs(sleep_time[count % sleep_time.len()])
    } else {
        Duration::from_secs(3)
    };
    let rs = scheduler.timeout(secs, move |s| {
        punch_request(
            s,
            context,
            nat_test,
            device_list,
            current_device,
            client_cipher,
            count + 1,
        );
    });
    if !rs {
        log::info!("定时任务停止");
    }
}

/// 随机对需要打洞的客户端发起打洞请求
fn punch0(
    context: &Context,
    nat_test: &NatTest,
    device_list: &Arc<Mutex<(u16, Vec<PeerDeviceInfo>)>>,
    current_device: CurrentDeviceInfo,
    client_cipher: &Cipher,
) -> io::Result<()> {
    let nat_info = nat_test.nat_info();
    let current_ip = current_device.virtual_ip;
    let mut list: Vec<PeerDeviceInfo> = device_list
        .lock()
        .1
        .iter()
        .filter(|info| info.status.is_online() && info.virtual_ip > current_ip)
        .cloned()
        .collect();
    list.shuffle(&mut rand::thread_rng());
    let mut count = 0;
    // // 优先没打洞的 need_punch会过滤掉已经打洞成功的
    // list.sort_by(|v1, v2| {
    //     if context.route_table.route_one_p2p(&v1.virtual_ip).is_none() {
    //         Ordering::Less
    //     } else if context.route_table.route_one_p2p(&v2.virtual_ip).is_none() {
    //         Ordering::Greater
    //     } else {
    //         Ordering::Equal
    //     }
    // });
    for info in list {
        if !info.status.is_online() {
            continue;
        }
        if info.virtual_ip <= current_device.virtual_ip {
            continue;
        }
        if !context.route_table.need_punch(&info.virtual_ip) {
            continue;
        }
        count += 1;
        if count > 2 {
            break;
        }
        let packet = punch_packet(
            client_cipher,
            current_device.virtual_ip(),
            &nat_info,
            info.virtual_ip,
        )?;
        log::info!(
            "发起打洞协商请求，目标:{:?},{:?}",
            info.virtual_ip,
            nat_info
        );
        context.send_default(packet.buffer(), current_device.connect_server)?;
    }
    Ok(())
}

fn punch_packet(
    client_cipher: &Cipher,
    virtual_ip: Ipv4Addr,
    nat_info: &NatInfo,
    dest: Ipv4Addr,
) -> io::Result<NetPacket<Vec<u8>>> {
    let mut punch_reply = PunchInfo::new();
    punch_reply.reply = false;
    punch_reply.public_ip_list = nat_info
        .public_ips
        .iter()
        .map(|ip| u32::from_be_bytes(ip.octets()))
        .collect();
    punch_reply.public_port = nat_info.public_ports.first().map_or(0, |v| *v as u32);
    punch_reply.public_ports = nat_info.public_ports.iter().map(|e| *e as u32).collect();
    punch_reply.public_port_range = nat_info.public_port_range as u32;
    punch_reply.local_ip = u32::from(nat_info.local_ipv4().unwrap_or(Ipv4Addr::UNSPECIFIED));
    punch_reply.local_port = nat_info.udp_ports[0] as u32;
    punch_reply.tcp_port = nat_info.tcp_port as u32;
    punch_reply.udp_ports = nat_info.udp_ports.iter().map(|e| *e as u32).collect();
    if let Some(ipv6) = nat_info.ipv6 {
        punch_reply.ipv6_port = nat_info.udp_ports[0] as u32;
        punch_reply.ipv6 = ipv6.octets().to_vec();
    }
    punch_reply.nat_type = protobuf::EnumOrUnknown::new(PunchNatType::from(nat_info.nat_type));
    log::info!("请求打洞={:?}", punch_reply);
    let bytes = punch_reply
        .write_to_bytes()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("punch_packet {:?}", e)))?;
    let mut net_packet = NetPacket::new_encrypt(vec![0u8; 12 + bytes.len() + ENCRYPTION_RESERVED])?;
    net_packet.set_version(Version::V1);
    net_packet.set_protocol(Protocol::OtherTurn);
    net_packet.set_transport_protocol(other_turn_packet::Protocol::Punch.into());
    net_packet.first_set_ttl(MAX_TTL);
    net_packet.set_source(virtual_ip);
    net_packet.set_destination(dest);
    net_packet.set_payload(&bytes)?;
    client_cipher.encrypt_ipv4(&mut net_packet)?;
    Ok(net_packet)
}
