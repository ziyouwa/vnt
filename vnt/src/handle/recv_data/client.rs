use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use parking_lot::RwLock;
use protobuf::Message;

use packet::icmp::{vnt_icmp, Kind};
use packet::ip::ipv4;
use packet::ip::ipv4::packet::IpV4Packet;
use tun::device::IFace;
use tun::Device;

use crate::channel::context::Context;
use crate::channel::punch::NatInfo;
use crate::channel::{Route, RouteKey};
use crate::cipher::Cipher;
use crate::external_route::AllowExternalRoute;
use crate::handle::maintain::PunchSender;
use crate::handle::recv_data::PacketHandler;
use crate::handle::CurrentDeviceInfo;
#[cfg(feature = "ip_proxy")]
use crate::ip_proxy::{IpProxyMap, ProxyHandler};
use crate::nat::NatTest;
use crate::proto::message::{PunchInfo, PunchNatType};
use crate::protocol::body::ENCRYPTION_RESERVED;
use crate::protocol::control_packet::ControlPacket;
use crate::protocol::{
    control_packet, ip_turn_packet, other_turn_packet, NetPacket, Protocol, Version, MAX_TTL,
};

/// 处理来源于客户端的包
#[derive(Clone)]
pub struct ClientPacketHandler {
    device: Arc<Device>,
    client_cipher: Cipher,
    punch_sender: PunchSender,
    peer_nat_info_map: Arc<RwLock<HashMap<Ipv4Addr, NatInfo>>>,
    nat_test: NatTest,
    route: AllowExternalRoute,
    #[cfg(feature = "ip_proxy")]
    ip_proxy_map: Option<IpProxyMap>,
}

impl ClientPacketHandler {
    pub fn new(
        device: Arc<Device>,
        client_cipher: Cipher,
        punch_sender: PunchSender,
        peer_nat_info_map: Arc<RwLock<HashMap<Ipv4Addr, NatInfo>>>,
        nat_test: NatTest,
        route: AllowExternalRoute,
        #[cfg(feature = "ip_proxy")] ip_proxy_map: Option<IpProxyMap>,
    ) -> Self {
        Self {
            device,
            client_cipher,
            punch_sender,
            peer_nat_info_map,
            nat_test,
            route,
            #[cfg(feature = "ip_proxy")]
            ip_proxy_map,
        }
    }
}

impl PacketHandler for ClientPacketHandler {
    fn handle(
        &self,
        mut net_packet: NetPacket<&mut [u8]>,
        route_key: RouteKey,
        context: &Context,
        current_device: &CurrentDeviceInfo,
    ) -> io::Result<()> {
        self.client_cipher.decrypt_ipv4(&mut net_packet)?;
        context
            .route_table
            .update_read_time(&net_packet.source(), &route_key);
        match net_packet.protocol() {
            Protocol::Service => {}
            Protocol::Error => {}
            Protocol::Control => {
                self.control(context, current_device, net_packet, route_key)?;
            }
            Protocol::IpTurn => {
                self.ip_turn(net_packet, context, current_device, route_key)?;
            }
            Protocol::OtherTurn => {
                self.other_turn(context, current_device, net_packet, route_key)?;
            }
            Protocol::Unknown(_) => {}
        }
        Ok(())
    }
}

impl ClientPacketHandler {
    fn ip_turn(
        &self,
        mut net_packet: NetPacket<&mut [u8]>,
        context: &Context,
        current_device: &CurrentDeviceInfo,
        route_key: RouteKey,
    ) -> io::Result<()> {
        let destination = net_packet.destination();
        let source = net_packet.source();
        match ip_turn_packet::Protocol::from(net_packet.transport_protocol()) {
            ip_turn_packet::Protocol::Ipv4 => {
                let mut ipv4 = IpV4Packet::new(net_packet.payload_mut())?;
                if let ipv4::protocol::Protocol::Icmp = ipv4.protocol() {
                    if ipv4.destination_ip() == destination {
                        let mut icmp_packet = vnt_icmp::IcmpPacket::new(ipv4.payload_mut())?;
                        if icmp_packet.kind() == Kind::EchoRequest {
                            //开启ping
                            icmp_packet.set_kind(Kind::EchoReply);
                            icmp_packet.update_checksum();
                            ipv4.set_source_ip(destination);
                            ipv4.set_destination_ip(source);
                            ipv4.update_checksum();
                            net_packet.set_source(destination);
                            net_packet.set_destination(source);
                            //不管加不加密，和接收到的数据长度都一致
                            self.client_cipher.encrypt_ipv4(&mut net_packet)?;
                            context.send_by_key(net_packet.buffer(), route_key)?;
                            return Ok(());
                        }
                    }
                }
                // ip代理只关心实际目标
                let real_dest = ipv4.destination_ip();
                if real_dest != destination
                    && !(real_dest.is_broadcast()
                        || real_dest.is_multicast()
                        || real_dest == current_device.broadcast_ip
                        || real_dest.is_unspecified())
                {
                    if !self.route.allow(&real_dest) {
                        //拦截不符合的目标
                        return Ok(());
                    }
                    #[cfg(feature = "ip_proxy")]
                    if let Some(ip_proxy_map) = &self.ip_proxy_map {
                        if ip_proxy_map.recv_handle(&mut ipv4, source, destination)? {
                            return Ok(());
                        }
                    }
                }
                self.device.write(net_packet.payload())?;
            }
            ip_turn_packet::Protocol::Ipv4Broadcast => {
                //客户端不帮忙转发广播包，所以不会出现这种类型的数据
            }
            ip_turn_packet::Protocol::Unknown(_) => {}
        }
        Ok(())
    }
    fn control(
        &self,
        context: &Context,
        current_device: &CurrentDeviceInfo,
        mut net_packet: NetPacket<&mut [u8]>,
        route_key: RouteKey,
    ) -> io::Result<()> {
        let metric = net_packet.source_ttl() - net_packet.ttl() + 1;
        let source = net_packet.source();
        match ControlPacket::new(net_packet.transport_protocol(), net_packet.payload())? {
            ControlPacket::PingPacket(_) => {
                net_packet.set_transport_protocol(control_packet::Protocol::Pong.into());
                net_packet.set_source(current_device.virtual_ip);
                net_packet.set_destination(source);
                net_packet.first_set_ttl(MAX_TTL);
                self.client_cipher.encrypt_ipv4(&mut net_packet)?;
                context.send_by_key(net_packet.buffer(), route_key)?;
                let route = Route::from_default_rt(route_key, metric);
                context.route_table.add_route_if_absent(source, route);
            }
            ControlPacket::PongPacket(pong_packet) => {
                let current_time = crate::handle::now_time() as u16;
                if current_time < pong_packet.time() {
                    return Ok(());
                }
                let rt = (current_time - pong_packet.time()) as i64;
                let route = Route::from(route_key, metric, rt);
                context.route_table.add_route(source, route);
            }
            ControlPacket::PunchRequest => {
                log::info!("PunchRequest={:?},source={}", route_key, source);
                if context.use_channel_type().is_only_relay() {
                    return Ok(());
                }
                //回应
                net_packet.set_transport_protocol(control_packet::Protocol::PunchResponse.into());
                net_packet.set_source(current_device.virtual_ip);
                net_packet.set_destination(source);
                net_packet.first_set_ttl(1);
                self.client_cipher.encrypt_ipv4(&mut net_packet)?;
                context.send_by_key(net_packet.buffer(), route_key)?;
                let route = Route::from_default_rt(route_key, 1);
                context.route_table.add_route_if_absent(source, route);
            }
            ControlPacket::PunchResponse => {
                log::info!("PunchResponse={:?},source={}", route_key, source);
                if context.use_channel_type().is_only_relay() {
                    return Ok(());
                }
                let route = Route::from_default_rt(route_key, 1);
                context.route_table.add_route_if_absent(source, route);
            }
            ControlPacket::AddrRequest => match route_key.addr.ip() {
                std::net::IpAddr::V4(ipv4) => {
                    let mut packet = NetPacket::new_encrypt([0; 12 + 6 + ENCRYPTION_RESERVED])?;
                    packet.set_version(Version::V1);
                    packet.set_protocol(Protocol::Control);
                    packet.set_transport_protocol(control_packet::Protocol::AddrResponse.into());
                    packet.first_set_ttl(MAX_TTL);
                    packet.set_source(current_device.virtual_ip);
                    packet.set_destination(source);
                    let mut addr_packet = control_packet::AddrPacket::new(packet.payload_mut())?;
                    addr_packet.set_ipv4(ipv4);
                    addr_packet.set_port(route_key.addr.port());
                    self.client_cipher.encrypt_ipv4(&mut packet)?;
                    context.send_by_key(packet.buffer(), route_key)?;
                }
                std::net::IpAddr::V6(_) => {}
            },
            ControlPacket::AddrResponse(_) => {}
        }
        Ok(())
    }
    fn other_turn(
        &self,
        context: &Context,
        current_device: &CurrentDeviceInfo,
        net_packet: NetPacket<&mut [u8]>,
        route_key: RouteKey,
    ) -> io::Result<()> {
        if context.use_channel_type().is_only_relay() {
            return Ok(());
        }
        let source = net_packet.source();
        match other_turn_packet::Protocol::from(net_packet.transport_protocol()) {
            other_turn_packet::Protocol::Punch => {
                let mut punch_info =
                    PunchInfo::parse_from_bytes(net_packet.payload()).map_err(|e| {
                        io::Error::new(io::ErrorKind::Other, format!("PunchInfo {:?}", e))
                    })?;
                let public_ips = punch_info
                    .public_ip_list
                    .iter()
                    .map(|v| Ipv4Addr::from(v.to_be_bytes()))
                    .collect();
                let local_ipv4 = Some(Ipv4Addr::from(punch_info.local_ip.to_be_bytes()));
                let tcp_port = punch_info.tcp_port as u16;
                let ipv6 = if punch_info.ipv6.len() == 16 {
                    let ipv6: [u8; 16] = punch_info.ipv6.try_into().unwrap();
                    Some(Ipv6Addr::from(ipv6))
                } else {
                    None
                };
                //兼容旧版本
                if punch_info.public_ports.is_empty() {
                    punch_info.public_ports.push(punch_info.public_port);
                }
                //兼容旧版本
                if punch_info.udp_ports.is_empty() {
                    punch_info.udp_ports.push(punch_info.local_port);
                }
                let peer_nat_info = NatInfo::new(
                    public_ips,
                    punch_info.public_ports.iter().map(|e| *e as u16).collect(),
                    punch_info.public_port_range as u16,
                    local_ipv4,
                    ipv6,
                    punch_info.udp_ports.iter().map(|e| *e as u16).collect(),
                    tcp_port,
                    punch_info.nat_type.enum_value_or_default().into(),
                );
                {
                    let peer_nat_info = peer_nat_info.clone();
                    self.peer_nat_info_map.write().insert(source, peer_nat_info);
                }
                if !punch_info.reply {
                    let mut punch_reply = PunchInfo::new();
                    punch_reply.reply = true;
                    let nat_info = self.nat_test.nat_info();
                    punch_reply.public_ip_list = nat_info
                        .public_ips
                        .iter()
                        .map(|ip| u32::from_be_bytes(ip.octets()))
                        .collect();
                    punch_reply.public_port = nat_info.public_ports.first().map_or(0, |v| *v as u32);
                    punch_reply.public_ports =
                        nat_info.public_ports.iter().map(|e| *e as u32).collect();
                    punch_reply.public_port_range = nat_info.public_port_range as u32;
                    punch_reply.tcp_port = nat_info.tcp_port as u32;
                    punch_reply.nat_type =
                        protobuf::EnumOrUnknown::new(PunchNatType::from(nat_info.nat_type));
                    punch_reply.local_ip =
                        u32::from(nat_info.local_ipv4().unwrap_or(Ipv4Addr::UNSPECIFIED));
                    punch_reply.local_port = nat_info.udp_ports[0] as u32;
                    punch_reply.udp_ports = nat_info.udp_ports.iter().map(|e| *e as u32).collect();
                    if let Some(ipv6) = nat_info.ipv6() {
                        punch_reply.ipv6 = ipv6.octets().to_vec();
                        punch_reply.ipv6_port = nat_info.udp_ports[0] as u32;
                    }
                    let bytes = punch_reply.write_to_bytes().map_err(|e| {
                        io::Error::new(io::ErrorKind::Other, format!("punch_reply {:?}", e))
                    })?;
                    let mut punch_packet =
                        NetPacket::new_encrypt(vec![0u8; 12 + bytes.len() + ENCRYPTION_RESERVED])?;
                    punch_packet.set_version(Version::V1);
                    punch_packet.set_protocol(Protocol::OtherTurn);
                    punch_packet.set_transport_protocol(other_turn_packet::Protocol::Punch.into());
                    punch_packet.first_set_ttl(MAX_TTL);
                    punch_packet.set_source(current_device.virtual_ip());
                    punch_packet.set_destination(source);
                    punch_packet.set_payload(&bytes)?;
                    self.client_cipher.encrypt_ipv4(&mut punch_packet)?;
                    if self.punch_sender.send(true, source, peer_nat_info) {
                        context.send_by_key(punch_packet.buffer(), route_key)?;
                    }
                } else {
                    self.punch_sender.send(false, source, peer_nat_info);
                }
            }
            other_turn_packet::Protocol::Unknown(e) => {
                log::warn!("不支持的转发协议 {:?},source:{:?}", e, source);
            }
        }
        Ok(())
    }
}
