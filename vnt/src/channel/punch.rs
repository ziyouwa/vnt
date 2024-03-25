use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::str::FromStr;
use std::time::Duration;
use std::{io, thread};

use mio::net::TcpStream;
use rand::prelude::SliceRandom;
use rand::Rng;

use crate::channel::context::Context;
use crate::channel::sender::AcceptSocketSender;

#[derive(Copy, Clone, Default, Eq, PartialEq, Debug)]
pub enum PunchModel {
    #[default]
    IPv4,
    IPv6,
    All,
}

impl FromStr for PunchModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "ipv4" => Ok(PunchModel::IPv4),
            "ipv6" => Ok(PunchModel::IPv6),
            "all" => Ok(PunchModel::All),
            _ => Err(format!("not match '{}', enum: ipv4/ipv6/all", s)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NatInfo {
    pub public_ips: Vec<Ipv4Addr>,
    pub public_ports: Vec<u16>,
    pub public_port_range: u16,
    pub nat_type: NatType,
    pub(crate) local_ipv4: Option<Ipv4Addr>,
    pub(crate) ipv6: Option<Ipv6Addr>,
    pub(crate) udp_ports: Vec<u16>,
    pub tcp_port: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum NatType {
    Symmetric,
    Cone,
}

impl NatInfo {
    pub fn new(
        mut public_ips: Vec<Ipv4Addr>,
        public_ports: Vec<u16>,
        public_port_range: u16,
        mut local_ipv4: Option<Ipv4Addr>,
        mut ipv6: Option<Ipv6Addr>,
        udp_ports: Vec<u16>,
        tcp_port: u16,
        mut nat_type: NatType,
    ) -> Self {
        public_ips.retain(|ip| {
            !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_unspecified()
                && !ip.is_loopback()
                && !ip.is_private()
        });
        if public_ips.len() > 1 {
            nat_type = NatType::Symmetric;
        }
        if let Some(ip) = local_ipv4 {
            if ip.is_multicast() || ip.is_broadcast() || ip.is_unspecified() || ip.is_loopback() {
                local_ipv4 = None
            }
        }
        if let Some(ip) = ipv6 {
            if ip.is_multicast() || ip.is_unspecified() || ip.is_loopback() {
                ipv6 = None
            }
        }
        Self {
            public_ips,
            public_ports,
            public_port_range,
            local_ipv4,
            ipv6,
            udp_ports,
            tcp_port,
            nat_type,
        }
    }
    pub fn update_addr(&mut self, index: usize, ip: Ipv4Addr, port: u16) {
        if port != 0 {
            if let Some(public_port) = self.public_ports.get_mut(index) {
                *public_port = port;
            }
        }
        if !ip.is_multicast()
            && !ip.is_broadcast()
            && !ip.is_unspecified()
            && !ip.is_loopback() && !ip.is_private() && !self.public_ips.contains(&ip) {
            self.public_ips.push(ip);
        }
    }
    pub fn local_ipv4(&self) -> Option<Ipv4Addr> {
        self.local_ipv4
    }
    pub fn ipv6(&self) -> Option<Ipv6Addr> {
        self.ipv6
    }
    pub fn local_udp_ipv4addr(&self, index: usize) -> Option<SocketAddr> {
        let len = self.udp_ports.len();
        if len == 0 {
            return None;
        }
        self.local_ipv4.map(|local_ipv4| SocketAddr::V4(SocketAddrV4::new(
                local_ipv4,
                self.udp_ports[index % len],
            )))
    }
    pub fn local_udp_ipv6addr(&self, index: usize) -> Option<SocketAddr> {
        let len = self.udp_ports.len();
        if len == 0 {
            return None;
        }
        self.ipv6.map(|ipv6| SocketAddr::V6(SocketAddrV6::new(
                ipv6,
                self.udp_ports[index % len],
                0,
                0,
            )))
    }

    pub fn local_tcp_ipv6addr(&self) -> Option<SocketAddr> {
        if self.tcp_port == 0 {
            return None;
        }
        self.ipv6.map(|ipv6| SocketAddr::V6(SocketAddrV6::new(ipv6, self.tcp_port, 0, 0)))
    }
    pub fn local_tcp_ipv4addr(&self) -> Option<SocketAddr> {
        if self.tcp_port == 0 {
            return None;
        }
        self.local_ipv4.map(|ipv4| SocketAddr::V4(SocketAddrV4::new(ipv4, self.tcp_port)))
    }
}

#[derive(Clone)]
pub struct Punch {
    context: Context,
    port_vec: Vec<u16>,
    port_index: HashMap<Ipv4Addr, usize>,
    punch_model: PunchModel,
    is_tcp: bool,
    tcp_socket_sender: AcceptSocketSender<(TcpStream, SocketAddr, Option<Vec<u8>>)>,
}

impl Punch {
    pub fn new(
        context: Context,
        punch_model: PunchModel,
        is_tcp: bool,
        tcp_socket_sender: AcceptSocketSender<(TcpStream, SocketAddr, Option<Vec<u8>>)>,
    ) -> Self {
        let mut port_vec: Vec<u16> = (1..65535).collect();
        port_vec.push(65535);
        let mut rng = rand::thread_rng();
        port_vec.shuffle(&mut rng);
        Punch {
            context,
            port_vec,
            port_index: HashMap::new(),
            punch_model,
            is_tcp,
            tcp_socket_sender,
        }
    }
}

impl Punch {
    fn connect_tcp(&self, buf: &[u8], addr: SocketAddr) -> bool {
        // mio是非阻塞的，不能立马判断是否能连接成功，所以用标准库的tcp
        match std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(tcp_stream) => {
                if tcp_stream.set_nonblocking(true).is_err() {
                    return false;
                }
                return self
                    .tcp_socket_sender
                    .try_add_socket((TcpStream::from_std(tcp_stream), addr, Some(buf.to_vec())))
                    .is_ok();
            }
            Err(e) => {
                log::warn!("连接到tcp失败,addr={},err={}", addr, e);
            }
        }
        false
    }
    pub fn punch(&mut self, buf: &[u8], id: Ipv4Addr, nat_info: NatInfo) -> io::Result<()> {
        if !self.context.route_table.need_punch(&id) {
            log::info!("已打洞成功,无需打洞:{:?}", id);
            return Ok(());
        }
        if self.is_tcp && nat_info.tcp_port != 0 {
            //向tcp发起连接
            if let Some(ipv6_addr) = nat_info.local_tcp_ipv6addr() {
                if self.connect_tcp(buf, ipv6_addr) {
                    return Ok(());
                }
            }
            //向tcp发起连接
            if let Some(ipv4_addr) = nat_info.local_tcp_ipv4addr() {
                if self.connect_tcp(buf, ipv4_addr) {
                    return Ok(());
                }
            }
            if nat_info.nat_type == NatType::Cone && nat_info.public_ips.len() == 1 {
                let addr =
                    SocketAddr::V4(SocketAddrV4::new(nat_info.public_ips[0], nat_info.tcp_port));
                if self.connect_tcp(buf, addr) {
                    return Ok(());
                }
            }
        }
        let channel_num = self.context.channel_num();
        for index in 0..channel_num {
            if let Some(ipv4_addr) = nat_info.local_udp_ipv4addr(index) {
                let _ = self.context.send_main_udp(index, buf, ipv4_addr);
            }
        }

        if self.punch_model != PunchModel::IPv4 {
            for index in 0..channel_num {
                if let Some(ipv6_addr) = nat_info.local_udp_ipv6addr(index) {
                    let rs = self.context.send_main_udp(index, buf, ipv6_addr);
                    log::info!("发送到ipv6地址:{:?},rs={:?}", ipv6_addr, rs);
                    if rs.is_ok() && self.punch_model == PunchModel::IPv6 {
                        return Ok(());
                    }
                }
            }
        }
        match nat_info.nat_type {
            NatType::Symmetric => {
                // 假设对方绑定n个端口，通过NAT对外映射出n个 公网ip:公网端口，自己随机尝试k次的情况下
                // 猜中的概率 p = 1-((65535-n)/65535)*((65535-n-1)/(65535-1))*...*((65535-n-k+1)/(65535-k+1))
                // n取76，k取600，猜中的概率就超过50%了
                // 前提 自己是锥形网络，否则猜中了也通信不了

                //预测范围内最多发送max_k1个包
                let max_k1 = 60;
                //全局最多发送max_k2个包
                let max_k2 = rand::thread_rng().gen_range(600..800);
                let port = nat_info.public_ports.first().copied().unwrap_or(0);
                if nat_info.public_port_range < max_k1 * 3 {
                    //端口变化不大时，在预测的范围内随机发送
                    let min_port = if port > nat_info.public_port_range {
                        port - nat_info.public_port_range
                    } else {
                        1
                    };
                    let (max_port, overflow) = port.overflowing_add(nat_info.public_port_range);
                    let max_port = if overflow { 65535 } else { max_port };
                    let k = if max_port - min_port + 1 > max_k1 {
                        max_k1 as usize
                    } else {
                        (max_port - min_port + 1) as usize
                    };
                    let mut nums: Vec<u16> = (min_port..max_port).collect();
                    nums.push(max_port);
                    nums.shuffle(&mut rand::thread_rng());
                    self.punch_symmetric(&nums[..k], buf, &nat_info.public_ips, max_k1 as usize)?;
                }
                let start = *self.port_index.entry(id).or_insert(0);
                let mut end = start + max_k2;
                if end > self.port_vec.len() {
                    end = self.port_vec.len();
                }
                let mut index = start
                    + self.punch_symmetric(
                        &self.port_vec[start..end],
                        buf,
                        &nat_info.public_ips,
                        max_k2,
                    )?;
                if index >= self.port_vec.len() {
                    index = 0
                }
                self.port_index.insert(id, index);
            }
            NatType::Cone => {
                let is_cone = self.context.is_cone();
                'a: for index in 0..nat_info.public_ports.len().min(channel_num) {
                    for ip in &nat_info.public_ips {
                        let port = nat_info.public_ports[index];
                        if port == 0 || ip.is_unspecified() {
                            continue;
                        }
                        let addr = SocketAddr::V4(SocketAddrV4::new(*ip, port));
                        if is_cone {
                            self.context.send_main_udp(index, buf, addr)?;
                        } else {
                            //只有一方是对称，则对称方要使用全部端口发送数据，符合上述计算的概率
                            self.context.try_send_all(buf, addr);
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    if !is_cone {
                        //对称网络数据只发一遍
                        break 'a;
                    }
                }
            }
        }
        Ok(())
    }

    fn punch_symmetric(
        &self,
        ports: &[u16],
        buf: &[u8],
        ips: &Vec<Ipv4Addr>,
        max: usize,
    ) -> io::Result<usize> {
        let mut count = 0;
        for (index, port) in ports.iter().enumerate() {
            for pub_ip in ips {
                count += 1;
                if count == max {
                    return Ok(index);
                }
                let addr = SocketAddr::V4(SocketAddrV4::new(*pub_ip, *port));
                self.context.send_main_udp(0, buf, addr)?;
                thread::sleep(Duration::from_millis(2));
            }
        }
        Ok(ports.len())
    }
}
