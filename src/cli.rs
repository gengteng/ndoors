use ndoors::*;

fn main() -> Result<()> {
    let doors = 3;
    let rounds = 100000;

    // 生成主持人
    let host = Uuid::new_v4();

    // 游戏设置
    let settings = Settings::new(doors, rounds);

    // 创建房间
    let mut room = Room::create(host, settings);

    // 生成挑战者
    let contestant = Uuid::new_v4();

    // 挑战者进房间对设置满意并点击就绪
    room.accept_contestant(contestant)?;
    room.contestant_ready(true)?;

    for _ in 0..room.settings().rounds {
        // 开始一轮随机游戏
        room.start_random()?;

        // 挑战者随机选择
        room.choose_random()?;

        // 主持人随机揭示
        room.reveal_random()?;

        // 挑战者随机做出抉择
        room.decide(rand::random())?;
    }

    // 完成本局游戏并获得每一轮的结果
    let results = room.complete(false)?;

    // 统计游戏结果
    let result = GameResult::calculate(settings.doors, results);
    let settings = result.settings();
    println!(
        "游戏设置: 共 {} 个门，进行了 {} 轮游戏；",
        settings.doors, settings.rounds
    );
    println!(
        "共赢得奖品 {} 轮，未赢得奖品 {} 轮，胜率 {:.2}%；",
        result.win(),
        settings.rounds - result.win(),
        result.win() as f64 * 100.0 / settings.rounds as f64
    );
    println!(
        "第一次就选择正确 {} 轮，第一次未选择正确 {} 轮，第一次选择正确率 {:.2}%；",
        result.chosen_win(),
        settings.rounds - result.chosen_win(),
        result.chosen_win() as f64 * 100.0 / settings.rounds as f64
    );
    println!(
        "坚持选择 {} 轮，坚持后赢得奖品 {} 轮，坚持选择正确率 {:.2}%；",
        result.stick(),
        result.stick_win(),
        result.stick_win() as f64 * 100.0 / result.stick() as f64
    );
    println!(
        "改变选择 {} 轮，改变后赢得奖品 {} 轮，改变选择正确率 {:.2}%。",
        result.switch(),
        result.switch_win(),
        result.switch_win() as f64 * 100.0 / result.switch() as f64
    );

    Ok(())
}
