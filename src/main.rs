use crate::room::{Room, Settings};
use error::*;
use uuid::Uuid;

mod error;
mod room;

fn main() -> Result<()> {
    // 生成主持人
    let host = Uuid::new_v4();

    // 游戏设置
    let settings = Settings::new(4, 20);

    // 创建房间
    let mut room = Room::create(host, settings);

    // 更新一下配置
    room.update_settings(Settings::new(5, 60))?;

    // 生成挑战者
    let contestant = Uuid::new_v4();

    // 挑战者进房间
    room.accept_contestant(contestant)?;

    // 这个时候还可以更新配置
    room.update_settings(Settings::new(5, 60))?;

    // 开始一句随机游戏
    let _prize = room.start_random();

    Ok(())
}
