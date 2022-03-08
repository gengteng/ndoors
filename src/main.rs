use crate::room::{Decision, Room, Settings};
use error::*;
use uuid::Uuid;

mod error;
mod room;

fn main() -> Result<()> {
    let doors = 7;
    let rounds = 80;

    // 生成主持人
    let host = Uuid::new_v4();
    println!("host: {host}");

    // 游戏设置
    let settings = Settings::new(doors, rounds);

    // 创建房间
    let mut room = Room::create(host, settings);
    println!("room: {:?}", room);

    // 生成挑战者
    let contestant = Uuid::new_v4();
    println!("contestant: {contestant}");

    // 挑战者进房间对设置满意并点击就绪
    room.accept_contestant(contestant)?;
    room.contestant_ready(true)?;
    println!("room: {:?}", room);

    for _ in 0..room.settings().rounds {
        // 开始一轮随机游戏
        room.start_random()?;

        // 挑战者随机选择
        room.choose_random()?;

        // 主持人随机揭示
        room.reveal_random()?;

        // 挑战者选择坚持
        room.decide(Decision::Switch)?;
    }

    for (index, result) in room.complete(false)?.iter().enumerate() {
        println!("{index}) {:?}", result);
    }

    Ok(())
}
