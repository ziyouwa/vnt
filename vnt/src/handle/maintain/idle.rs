use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_utils::atomic::AtomicCell;
use mio::net::TcpStream;

use crate::channel::context::Context;
use crate::channel::idle::{Idle, IdleType};
use crate::channel::sender::AcceptSocketSender;
use crate::handle::callback::{ConnectInfo, ErrorType};
use crate::handle::handshaker::Handshake;
use crate::handle::{handshaker, BaseConfigInfo, ConnectStatus, CurrentDeviceInfo};
use crate::util::Scheduler;
use crate::{ErrorInfo, VntCallback};

pub fn idle_route<Call: VntCallback>(
    scheduler: &Scheduler,
    idle: Idle,
    context: Context,
    current_device_info: Arc<AtomicCell<CurrentDeviceInfo>>,
    call: Call,
) {
    let delay = idle_route0(&idle, &context, &current_device_info, &call);
    let rs = scheduler.timeout(delay, move |s| {
        idle_route(s, idle, context, current_device_info, call)
    });
    if !rs {
        log::info!("定时任务停止");
    }
}
pub fn idle_gateway<Call: VntCallback>(
    scheduler: &Scheduler,
    context: Context,
    current_device_info: Arc<AtomicCell<CurrentDeviceInfo>>,
    config: BaseConfigInfo,
    tcp_socket_sender: AcceptSocketSender<(TcpStream, SocketAddr, Option<Vec<u8>>)>,
    call: Call,
    connect_count: usize,
    handshake: Handshake,
) {
    let time = Instant::now();
    idle_gateway_(
        scheduler,
        context,
        current_device_info,
        config,
        tcp_socket_sender,
        call,
        connect_count,
        handshake,
        time,
    );
}
pub fn idle_gateway_<Call: VntCallback>(
    scheduler: &Scheduler,
    context: Context,
    current_device_info: Arc<AtomicCell<CurrentDeviceInfo>>,
    config: BaseConfigInfo,
    tcp_socket_sender: AcceptSocketSender<(TcpStream, SocketAddr, Option<Vec<u8>>)>,
    call: Call,
    mut connect_count: usize,
    handshake: Handshake,
    mut time: Instant,
) {
    idle_gateway0(
        &context,
        &current_device_info,
        &config,
        &tcp_socket_sender,
        &call,
        &mut connect_count,
        &handshake,
        &mut time,
    );
    let rs = scheduler.timeout(Duration::from_secs(5), move |s| {
        idle_gateway_(
            s,
            context,
            current_device_info,
            config,
            tcp_socket_sender,
            call,
            connect_count,
            handshake,
            time,
        )
    });
    if !rs {
        log::info!("定时任务停止");
    }
}
fn idle_gateway0<Call: VntCallback>(
    context: &Context,
    current_device: &AtomicCell<CurrentDeviceInfo>,
    config: &BaseConfigInfo,
    tcp_socket_sender: &AcceptSocketSender<(TcpStream, SocketAddr, Option<Vec<u8>>)>,
    call: &Call,
    connect_count: &mut usize,
    handshake: &Handshake,
    time: &mut Instant,
) {
    if let Err(e) = check_gateway_channel(
        context,
        current_device,
        config,
        tcp_socket_sender,
        call,
        connect_count,
        handshake,
        time,
    ) {
        let cur = current_device.load();
        call.error(ErrorInfo::new_msg(
            ErrorType::Disconnect,
            format!("connect:{},error:{:?}", cur.connect_server, e),
        ));
    }
}
fn idle_route0<Call: VntCallback>(
    idle: &Idle,
    context: &Context,
    current_device: &AtomicCell<CurrentDeviceInfo>,
    call: &Call,
) -> Duration {
    let cur = current_device.load();
    match idle.next_idle() {
        IdleType::Timeout(ip, route) => {
            log::info!("route Timeout {:?},{:?}", ip, route);
            context.remove_route(&ip, route.route_key());
            if cur.is_gateway(&ip) {
                //网关路由过期，则需要改变状态
                crate::handle::change_status(current_device, ConnectStatus::Connecting);
                call.error(ErrorInfo::new(ErrorType::Disconnect));
            }
            Duration::from_millis(100)
        }
        IdleType::Sleep(duration) => duration,
        IdleType::None => Duration::from_millis(3000),
    }
}

fn check_gateway_channel<Call: VntCallback>(
    context: &Context,
    current_device_info: &AtomicCell<CurrentDeviceInfo>,
    config: &BaseConfigInfo,
    tcp_socket_sender: &AcceptSocketSender<(TcpStream, SocketAddr, Option<Vec<u8>>)>,
    call: &Call,
    count: &mut usize,
    handshake: &Handshake,
    time: &mut Instant,
) -> io::Result<()> {
    let mut current_device = current_device_info.load();
    if current_device.status.offline() {
        *count += 1;
        if time.elapsed() < Duration::from_secs(6 * 60) {
            // 探测服务器地址
            current_device = domain_request0(current_device_info, config);
            *time = Instant::now()
        }
        //需要重连
        call.connect(ConnectInfo::new(*count, current_device.connect_server));
        log::info!("发送握手请求,{:?}", config);
        if let Err(e) = handshake.send(context, config.client_secret, current_device.connect_server)
        {
            log::warn!("{:?}", e);
            if context.is_main_tcp() {
                let request_packet = handshaker::handshake_request_packet(config.client_secret)?;
                //tcp需要重连
                let tcp_stream = std::net::TcpStream::connect_timeout(
                    &current_device.connect_server,
                    Duration::from_secs(5),
                )?;
                tcp_stream.set_nonblocking(true)?;
                if let Err(e) = tcp_socket_sender.try_add_socket((
                    TcpStream::from_std(tcp_stream),
                    current_device.connect_server,
                    Some(request_packet.into_buffer()),
                )) {
                    log::warn!("{:?}", e)
                }
            }
        }
    }
    Ok(())
}
pub fn domain_request0(
    current_device: &AtomicCell<CurrentDeviceInfo>,
    config: &BaseConfigInfo,
) -> CurrentDeviceInfo {
    let mut current_dev = current_device.load();
    // 探测服务端地址变化
    if let Ok(mut addr) = config.server_addr.to_socket_addrs() {
        if let Some(addr) = addr.next() {
            if addr != current_dev.connect_server {
                let mut tmp = current_dev;
                tmp.connect_server = addr;
                let rs = current_device.compare_exchange(current_dev, tmp);
                current_dev.connect_server = addr;
                log::info!(
                    "服务端地址变化,旧地址:{}，新地址:{},替换结果:{}",
                    current_dev.connect_server,
                    addr,
                    rs.is_ok()
                );
            }
        }
    }
    current_dev
}
