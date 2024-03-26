use std::io;
use vnt::core::Vnt;

use crate::command::entity::{DeviceItem, Info, RouteItem};
use crate::console_out;

pub mod client;
pub mod entity;
pub mod server;

pub enum CommandEnum {
    Route,
    List,
    All,
    Info,
    Stop,
}

pub fn command(cmd: CommandEnum) {
    if let Err(e) = command_(cmd) {
        println!("cmd: {:?}", e);
    }
}

fn command_(cmd: CommandEnum) -> io::Result<()> {
    let mut command_client = client::CommandClient::new()?;
    match cmd {
        CommandEnum::Route => {
            let list = command_client.route()?;
            console_out::console_route_table(list);
        }
        CommandEnum::List => {
            let list = command_client.list()?;
            console_out::console_device_list(list);
        }
        CommandEnum::All => {
            let list = command_client.list()?;
            console_out::console_device_list_all(list);
        }
        CommandEnum::Info => {
            let info = command_client.info()?;
            console_out::console_info(info);
        }
        CommandEnum::Stop => {
            command_client.stop()?;
        }
    }
    Ok(())
}

pub fn command_route(vnt: &Vnt) -> Vec<RouteItem> {
    let route_table = vnt.route_table();
    let mut route_list = Vec::with_capacity(route_table.len());
    for (destination, routes) in route_table {
        for route in routes {
            let next_hop = vnt
                .route_key(&route.route_key())
                .map_or(String::new(), |v| v.to_string());
            let metric = route.metric.to_string();
            let rt = if route.rt < 0 {
                "".to_string()
            } else {
                route.rt.to_string()
            };
            let interface = if route.is_tcp {
                format!("tcp@{}", route.addr)
            } else {
                route.addr.to_string()
            };
            let item = RouteItem {
                destination: destination.to_string(),
                next_hop,
                metric,
                rt,
                interface,
            };
            route_list.push(item);
        }
    }
    route_list
}

pub fn command_list(vnt: &Vnt) -> Vec<DeviceItem> {
    let info = vnt.current_device();
    let device_list = vnt.device_list();
    let mut list = Vec::new();
    let current_client_secret = vnt.client_encrypt();
    for peer in device_list {
        let name = peer.name;
        let virtual_ip = peer.virtual_ip.to_string();
        let (nat_type, public_ips, local_ip, ipv6) =
            if let Some(nat_info) = vnt.peer_nat_info(&peer.virtual_ip) {
                let nat_type = format!("{:?}", nat_info.nat_type);
                let public_ips: Vec<String> =
                    nat_info.public_ips.iter().map(|v| v.to_string()).collect();
                let public_ips = public_ips.join(",");
                let local_ip = nat_info
                    .local_ipv4()
                    .map(|v| v.to_string())
                    .unwrap_or("None".to_string());
                let ipv6 = nat_info
                    .ipv6()
                    .map(|v| v.to_string())
                    .unwrap_or("None".to_string());
                (nat_type, public_ips, local_ip, ipv6)
            } else {
                (
                    "".to_string(),
                    "".to_string(),
                    "".to_string(),
                    "".to_string(),
                )
            };
        let (nat_traversal_type, rt) = if let Some(route) = vnt.route(&peer.virtual_ip) {
            let nat_traversal_type = if route.metric == 1 {
                if route.is_tcp {
                    "tcp-p2p"
                } else {
                    "p2p"
                }
            } else {
                let next_hop = vnt.route_key(&route.route_key());
                if let Some(next_hop) = next_hop {
                    if info.is_gateway(&next_hop) {
                        "server-relay"
                    } else {
                        "client-relay"
                    }
                } else {
                    "server-relay"
                }
            }
            .to_string();
            let rt = if route.rt < 0 {
                "".to_string()
            } else {
                route.rt.to_string()
            };
            (nat_traversal_type, rt)
        } else {
            ("relay".to_string(), "".to_string())
        };
        let status = format!("{:?}", peer.status);
        let client_secret = peer.client_secret;
        let item = DeviceItem {
            name,
            virtual_ip,
            nat_type,
            public_ips,
            local_ip,
            ipv6,
            nat_traversal_type,
            rt,
            status,
            client_secret,
            current_client_secret,
        };
        list.push(item);
    }
    list
}

pub fn command_info(vnt: &Vnt) -> Info {
    let current_device = vnt.current_device();
    let nat_info = vnt.nat_info();
    let name = vnt.name().to_string();
    let virtual_ip = current_device.virtual_ip().to_string();
    let virtual_gateway = current_device.virtual_gateway().to_string();
    let virtual_netmask = current_device.virtual_netmask.to_string();
    let connect_status = format!("{:?}", vnt.connection_status());
    let relay_server = current_device.connect_server.to_string();
    let nat_type = format!("{:?}", nat_info.nat_type);
    let public_ips: Vec<String> = nat_info.public_ips.iter().map(|v| v.to_string()).collect();
    let public_ips = public_ips.join(",");
    let local_addr = nat_info
        .local_ipv4()
        .map(|v| v.to_string())
        .unwrap_or("None".to_string());
    let ipv6_addr = nat_info
        .ipv6()
        .map(|v| v.to_string())
        .unwrap_or("None".to_string());
    let up = vnt.up_stream();
    let down = vnt.down_stream();
    Info {
        name,
        virtual_ip,
        virtual_gateway,
        virtual_netmask,
        connect_status,
        relay_server,
        nat_type,
        public_ips,
        local_addr,
        ipv6_addr,
        up,
        down,
    }
}
