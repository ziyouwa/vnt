use std::{io, path::PathBuf};

use clap::Parser;

use crate::cli::VntArgs;
use vnt::core;

mod cli;
mod command;
mod config;
mod root_check;
mod console_out;

pub fn app_home() -> io::Result<PathBuf> {
    let root_path = match std::env::current_exe() {
        Ok(path) => {
            if let Some(v) = path.as_path().parent() {
                v.to_path_buf()
            } else {
                log::warn!("current_exe parent none:{:?}", path);
                PathBuf::new()
            }
        }
        Err(e) => {
            log::warn!("current_exe err:{:?}", e);
            PathBuf::new()
        }
    };
    let path = root_path.join("env");
    if !path.exists() {
        std::fs::create_dir_all(&path)?;
    }
    Ok(path)
}

fn main() {
    // 程序需要root权限才能正常运行
    if !root_check::is_app_elevated() {
        println!("Please run it with administrator or root privileges");
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        sudo::escalate_if_needed().unwrap();
        return;
    }

    // 初始化日志
    let _ = log4rs::init_file("log4rs.yaml", Default::default());

    let args = VntArgs::parse();
    println!("{:?}", args);

    //  此处开始处理各种命令行参数，以if let的形式

    //  后台运行时，执行命令
    if let Some(cmd) = args.command {
        match cmd {
            cli::Commands::Peer { list, info, all } => {
                if list {
                    command::command(command::CommandEnum::List);
                }
                if info {
                    command::command(command::CommandEnum::Info);
                }
                if all {
                    command::command(command::CommandEnum::All);
                }
            }
            cli::Commands::Route { print } => command::command(command::CommandEnum::Route),
            cli::Commands::Service { stop } => command::command(command::CommandEnum::Stop),
        }
        return;
    }

    // 从配置文件读取配置
    if let Some(conf) = args.config {
        match config::read_config(conf.to_str().unwrap()) {
            Ok((cfg, cmd)) => main0(cfg, cmd),
            Err(e) => {
                eprintln!("Reading config error: {:?}", e);
                return;
            }
        }
    }

    // 获取token
    if let Some(token) = args.token {}
}

fn main0(cfg: core::Config, cmd: bool) {
    todo!()
}
