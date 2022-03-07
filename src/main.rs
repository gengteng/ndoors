use crate::room::{Decision, Room, Settings};
use error::*;
use uuid::Uuid;

mod error;
mod room;

fn main() -> Result<()> {
    // 生成主持人
    let host = Uuid::new_v4();
    println!("host: {host}");

    // 游戏设置
    let settings = Settings::new(4, 20);

    // 创建房间
    let mut room = Room::create(host, settings);
    println!("room: {:?}", room);

    // 更新一下配置
    room.update_settings(Settings::new(5, 60))?;
    println!("room: {:?}", room);

    // 生成挑战者
    let contestant = Uuid::new_v4();
    println!("host: {contestant}");

    // 挑战者进房间对设置满意并点击就绪
    room.accept_contestant(contestant)?;
    room.contestant_ready(true)?;
    println!("room: {:?}", room);

    // 这个时候还可以更新配置
    room.update_settings(Settings::new(8, 60))?;
    println!("room: {:?}", room);

    // 挑战者再次选择就绪
    room.contestant_ready(true)?;

    // 开始一轮随机游戏
    let prize = room.start_random()?;
    println!("prize: {}", prize);

    // 挑战者随机选择
    let chosen = room.choose_random()?;
    println!("chosen: {}", chosen);

    // 主持人随机揭示
    let left = room.reveal_random()?;
    println!("left: {}", left);

    // 挑战者选择坚持
    let result = room.decide(Decision::Switch)?;
    println!("result: {:?}", result);

    Ok(())
}
