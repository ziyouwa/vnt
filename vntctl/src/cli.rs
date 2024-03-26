use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
pub enum Cipher {
    #[cfg(any(feature = "aes_gcm", feature = "server_encrypt"))]
    AesGcm,
    #[cfg(feature = "aes_cbc")]
    AesCbc,
    #[cfg(feature = "aes_ecb")]
    AesEcb,
    #[cfg(feature = "sm4_cbc")]
    Sm4Cbc,
}

#[derive(Debug, Parser, Serialize, Deserialize)]
#[command(version, about, long_about = None)]
pub struct VntArgs {
    /// 使用相同的token,就能组建一个局域网络
    #[arg(short = 'k', long, value_name = "token")]
    pub token: Option<String>,

    /// 给设备一个名字，便于区分不同设备，默认使用系统版本
    #[arg(short, long, value_name = "name")]
    pub name: Option<String>,

    /// 设备唯一标识符，不使用--ip参数时，服务端凭此参数分配虚拟ip，注意不能重复
    #[arg( long, value_name = "id")]
    pub id: Option<String>,

    /// 注册和中继服务器地址
    #[arg(short, long, value_name = "server")]
    pub server: Option<String>,

    /// stun服务器，用于探测NAT类型，可多次指定，如-e addr1 -e addr2
    #[arg(short = 't', long, value_name = "stun-server")]
    pub stun_server: Vec<String>,

    /// 使用tap模式，默认使用tun模式
    #[arg(long, default_value_t = false)]
    pub tap: bool,

    /// 配置点对网(IP代理)时使用,-i 192.168.0.0/24,10.26.0.3表示允许接收网段192.168.0.0/24的数据并转发到10.26.0.3,可指定多个网段
    #[arg(short, long, value_name = "in-ip")]
    pub in_ip: Option<String>,

    #[cfg(feature = "ip_proxy")]
    /// 配置点对网时使用,-o 192.168.0.0/24表示允许将数据转发到192.168.0.0/24,可指定多个网段
    #[arg(short, long, value_name = "out-ip")]
    pub out_ip: Option<String>,
            
    /// 使用该密码生成的密钥对客户端数据进行加密,并且服务端无法解密,使用相同密码的客户端才能通信
    #[arg(short = 'p', long, value_name = "password")]
    pub password: Option<String>,

    #[cfg(feature = "server_encrypt")]
    /// 加密当前客户端和服务端通信的数据,请留意服务端指纹是否正确
    #[arg(short = 'r', long, default_value_t = false)]
    pub server_encrypt: bool,
    
    /// 自定义mtu(不加密默认为1450，加密默认为1410)
    #[arg( value_name="mtu", long, default_value_t = 1450)]
    pub mtu: u16,

    /// 和服务端使用tcp通信,默认使用udp,遇到udp qos时可指定使用tcp
    #[arg( long, default_value_t = false)]
    pub tcp: bool,

    /// 指定虚拟ip,指定的ip不能和其他设备重复,必须有效并且在服务端所属网段下,默认情况由服务端分配
    #[arg(value_name="ip", long)]
    pub ip: Option<String>,

    /// 任务并行度(必须为正整数),默认值为1
    #[arg( short = 'T' ,value_name="parallel", long, default_value_t = 1, value_parser= clap::value_parser!(u16).range(1..))]
    pub parallel: u16,

    /// 指定加密算法
    #[arg(short = 'm', value_name="cipher", long, value_enum, default_value = "aes-gcm")]
    pub cipher: Option<Cipher>,

    /// 增加数据指纹校验,可增加安全性,如果服务端开启指纹校验,则客户端也必须开启
    #[arg(short = 'z', long, default_value_t = false)]
    pub fingerprint: bool,

    /// 取值ipv4/ipv6/all,ipv4表示仅使用ipv4打洞
    #[arg(short = 'u', value_name="punch", long)]
    pub punch: Option<String>,

    ///  取值0~65535，指定本地监听的一组端口,默认监听两个随机端口,使用过多端口会增加网络负担
    #[arg(value_name="port", long, value_parser= clap::value_parser!(u16).range(0..65536))]
    pub ports: Option<Vec<u16>>,

    #[cfg(feature = "ip_proxy")]
    /// 关闭内置代理，如需点对网则需要配置网卡NAT转发
    #[arg(long, default_value_t = false)]
    pub no_proxy: bool,

    /// 优先低延迟的通道，默认情况优先使用p2p通道
    #[arg(short, long, default_value_t = false)]
    pub first_latency: bool,

    /// 使用通道 relay/p2p/all,默认两者都使用
    #[arg(short='e', long, value_name = "p2p", default_value = "all")]
    pub use_channel: Option<String>,

    /// 指定虚拟网卡名称
    #[arg( long, value_name = "tun0")]
    pub nic: Option<String>,

    /// 模拟丢包，取值0~1之间的小数，程序会按设定的概率主动丢包，可用于模拟弱网
    #[arg(short='l', long, value_name = "0")]
    pub packet_loss: Option<f32>,

    /// 模拟延迟，单位毫秒，程序会按设定的延迟主动延迟，可用于模拟弱网
    #[arg(short='d', long, value_name = "0")]
    pub packet_delay: Option<u64>,

    /// 读取配置文件，配置文件格式参考配置文件
    #[arg(short, long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// 开启交互式命令,使用此参数开启控制台输入
    #[arg(long, default_value_t = false)]
    pub cmd: bool,

    /// 后台运行时的子命令
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, clap::Subcommand, Serialize, Deserialize)]
pub enum Commands {
    /// 查看设备信息
    Peer {
        /// 查看其他设备列表
        #[arg(short, long)]
        list: bool,
        /// 查看当前设备信息
        #[arg(short, long)]
        info: bool,
        /// 查看其他设备信息
        #[arg(short, long)]
        all: bool
    }, 
    /// 查看数据转发信息
    Route {
        /// 查看数据转发路径
        #[arg(short, long)]
        print: bool
    },
    /// 后台服务控制
    Service {
        /// 停止后台服务
        #[arg(short, long)]
        stop: bool
    }
}

#[test]
fn feature() {
    let args = VntArgs::parse();
    println!("{:?}",args);
}