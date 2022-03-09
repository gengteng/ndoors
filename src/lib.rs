mod error;

pub use error::*;
use rand::distributions::Standard;
use rand::prelude::Distribution;
pub use uuid::Uuid;

use rand::Rng;
use serde::{Deserialize, Serialize};

/// 房间状态
#[derive(Debug, Serialize, Deserialize)]
pub enum RoomState {
    /// 刚刚创建
    Created,

    /// 挑战者已加入
    Joined {
        /// 挑战者 ID
        contestant: Uuid,

        /// 挑战者已准备好开始
        ready: bool,
    },

    /// 游戏已开始
    Started {
        /// 挑战者 ID
        contestant: Uuid,

        /// 当前游戏轮数
        current_round: u32,

        /// 当前轮游戏奖品所在门序号
        prize: u32,

        /// 当前已经赢的轮数
        results: Vec<RoundResult>,

        /// 当前轮状态
        stage: Stage,
    },
}

impl Default for RoomState {
    fn default() -> Self {
        Self::Created
    }
}

/// 一轮游戏的各个阶段
#[derive(Debug, Serialize, Deserialize)]
pub enum Stage {
    /// 挑战者选择
    Choose,

    /// 主持人揭示
    Reveal {
        /// 挑战者已经选择的门序号
        chosen: u32,
    },

    /// 挑战者抉择
    Decide {
        /// 挑战者已经选择的门序号
        chosen: u32,

        /// 主持人揭示后留给挑战者的门序号
        left: u32,
    },

    /// 游戏结束
    End { result: RoundResult },
}

impl Default for Stage {
    fn default() -> Self {
        Self::Choose
    }
}

impl Stage {
    pub fn is_end(&self) -> bool {
        matches!(self, Stage::End { .. })
    }
}

/// 一轮游戏的结果
#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub struct RoundResult {
    /// 奖品所在门序号
    prize: u32,

    /// 挑战者选择门序号
    chosen: u32,

    /// 主持人揭示后剩下的门序号
    left: u32,

    /// 挑战者的抉择
    decision: Decision,

    /// 是否赢的奖品
    win: bool,
}

/// 游戏设置
#[derive(Debug, Serialize, Deserialize, Copy, Clone, Eq, PartialEq)]
pub struct Settings {
    /// 门数
    pub doors: u32,

    /// 轮数
    pub rounds: u32,
}

impl Settings {
    pub fn new(doors: u32, rounds: u32) -> Self {
        Self { doors, rounds }
    }
}

/// 挑战者抉择
#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub enum Decision {
    /// 改变选择
    Switch,

    /// 坚持选择
    Stick,
}

impl Default for Decision {
    fn default() -> Self {
        Self::Switch
    }
}

impl Distribution<Decision> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Decision {
        if 0 > rng.next_u32() as i32 {
            Decision::Switch
        } else {
            Decision::Stick
        }
    }
}

/// 游戏房间
#[derive(Debug, Serialize, Deserialize)]
pub struct Room {
    /// 房间 ID
    id: Uuid,
    /// 主持人 ID
    host: Uuid,
    /// 游戏设置
    settings: Settings,
    /// 房间状态
    state: RoomState,
}

impl Room {
    /// 创建房间
    pub fn create(host: Uuid, settings: Settings) -> Self {
        Self {
            id: Uuid::new_v4(),
            host,
            settings,
            state: RoomState::default(),
        }
    }

    /// 房间 ID
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// 主持人 ID
    pub fn host(&self) -> &Uuid {
        &self.host
    }

    /// 当前游戏配置
    pub fn settings(&self) -> Settings {
        self.settings
    }

    /// 当前房间状态
    pub fn state(&self) -> &RoomState {
        &self.state
    }

    /// 接收挑战者
    pub fn accept_contestant(&mut self, contestant: Uuid) -> Result<()> {
        if let RoomState::Created = self.state {
            self.state = RoomState::Joined {
                contestant,
                ready: false,
            };
            Ok(())
        } else {
            Err(Error::InvalidOperation)
        }
    }

    /// 踢出挑战者
    pub fn kick_contestant(&mut self) -> Result<()> {
        if matches!(
            self.state,
            RoomState::Joined { .. } | RoomState::Started { .. }
        ) {
            self.state = RoomState::Created;
            Ok(())
        } else {
            Err(Error::InvalidOperation)
        }
    }

    /// 挑战者就绪
    pub fn contestant_ready(&mut self, ready: bool) -> Result<()> {
        match &mut self.state {
            RoomState::Joined { ready: r, .. } => {
                *r = ready;
                Ok(())
            }
            _ => Err(Error::InvalidOperation),
        }
    }

    /// 更新设置，返回 `true` 表示需要通知挑战者重新选择就绪
    pub fn update_settings(&mut self, settings: Settings) -> Result<bool> {
        match &mut self.state {
            RoomState::Created => {
                self.settings = settings;
                Ok(false)
            }
            RoomState::Joined { ready, .. } => {
                // 如果配置没有改变，不需要做任何事
                let notify_contestant = self.settings != settings;
                // 如果挑战者已经就绪，需要重置，让挑战者重新选择就绪
                if notify_contestant {
                    self.settings = settings;
                    *ready = false;
                }
                Ok(notify_contestant)
            }
            RoomState::Started { .. } => Err(Error::InvalidOperation),
        }
    }

    /// 开始游戏并将奖品随机放到一个门内
    pub fn start_random(&mut self) -> Result<u32> {
        match &mut self.state {
            RoomState::Joined { ready, contestant } if *ready => {
                let prize = rand::thread_rng().gen_range(0..self.settings.doors);
                self.state = RoomState::Started {
                    contestant: *contestant,
                    current_round: 0,
                    prize,
                    results: vec![],
                    stage: Stage::Choose,
                };
                Ok(prize)
            }
            RoomState::Started {
                current_round,
                stage,
                prize,
                ..
            } if stage.is_end() && *current_round < self.settings.rounds - 1 => {
                let new_prize = rand::thread_rng().gen_range(0..self.settings.doors);
                *current_round += 1;
                *stage = Stage::Choose;
                *prize = new_prize;
                Ok(new_prize)
            }
            _ => Err(Error::InvalidOperation),
        }
    }

    /// 开始游戏并将奖品放到序号指定的门内
    pub fn start(&mut self, prize: u32) -> Result<()> {
        if prize >= self.settings.doors {
            return Err(Error::InvalidDoorIndex);
        }
        match &mut self.state {
            RoomState::Joined { ready, contestant } if *ready => {
                self.state = RoomState::Started {
                    contestant: *contestant,
                    current_round: 0,
                    prize,
                    results: vec![],
                    stage: Stage::Choose,
                };
                Ok(())
            }
            RoomState::Started {
                current_round,
                prize: p,
                stage,
                ..
            } if stage.is_end() && *current_round < self.settings.rounds - 1 => {
                *current_round += 1;
                *stage = Stage::Choose;
                *p = prize;
                Ok(())
            }
            _ => Err(Error::InvalidOperation),
        }
    }

    /// 挑战者随机选择
    pub fn choose_random(&mut self) -> Result<u32> {
        match &mut self.state {
            RoomState::Started { stage, .. } => {
                if let Stage::Choose = stage {
                    let chosen = rand::thread_rng().gen_range(0..self.settings.doors);
                    *stage = Stage::Reveal { chosen };
                    Ok(chosen)
                } else {
                    Err(Error::InvalidOperation)
                }
            }
            _ => Err(Error::InvalidOperation),
        }
    }

    /// 挑战者做出选择
    pub fn choose(&mut self, chosen: u32) -> Result<()> {
        if chosen >= self.settings.doors {
            return Err(Error::InvalidDoorIndex);
        }

        match &mut self.state {
            RoomState::Started { stage, .. } => {
                if let Stage::Choose = stage {
                    *stage = Stage::Reveal { chosen };
                    Ok(())
                } else {
                    Err(Error::InvalidOperation)
                }
            }
            _ => Err(Error::InvalidOperation),
        }
    }

    /// 主持人揭示（提供留下的门序号即可）
    pub fn reveal_random(&mut self) -> Result<u32> {
        match &mut self.state {
            RoomState::Started { stage, prize, .. } => {
                if let Stage::Reveal { chosen } = stage {
                    let left = if *chosen == *prize {
                        random_door(self.settings.doors, *chosen)
                    } else {
                        *prize
                    };

                    *stage = Stage::Decide {
                        chosen: *chosen,
                        left,
                    };
                    Ok(left)
                } else {
                    Err(Error::InvalidOperation)
                }
            }
            _ => Err(Error::InvalidOperation),
        }
    }

    /// 主持人揭示（提供留下的门序号即可）
    pub fn reveal(&mut self, left: u32) -> Result<()> {
        if left >= self.settings.doors {
            return Err(Error::InvalidDoorIndex);
        }

        match &mut self.state {
            RoomState::Started { stage, prize, .. } => {
                if let Stage::Reveal { chosen } = stage {
                    // 1. 不可能留下挑战者已经选择的那个门；
                    // 2. 如果挑战者选择的不是奖，则留下的必须是奖，否则主持人打开的门中就有奖了
                    if left == *chosen || (*chosen != *prize && left != *prize) {
                        Err(Error::InvalidOperation)
                    } else {
                        *stage = Stage::Decide {
                            chosen: *chosen,
                            left,
                        };
                        Ok(())
                    }
                } else {
                    Err(Error::InvalidOperation)
                }
            }
            _ => Err(Error::InvalidOperation),
        }
    }

    /// 挑战者做出最终抉择
    pub fn decide(&mut self, decision: Decision) -> Result<RoundResult> {
        if let RoomState::Started {
            prize,
            ref mut results,
            stage,
            ..
        } = &mut self.state
        {
            let result = match stage {
                Stage::Decide { chosen, left } => {
                    let win_the_prize = matches!((*chosen, *left, decision), (p, _, Decision::Stick) | (_, p, Decision::Switch) if p == *prize);
                    RoundResult {
                        prize: *prize,
                        chosen: *chosen,
                        left: *left,
                        decision,
                        win: win_the_prize,
                    }
                }
                _ => return Err(Error::InvalidOperation),
            };

            results.push(result);
            *stage = Stage::End { result };
            Ok(result)
        } else {
            Err(Error::InvalidOperation)
        }
    }

    /// 完成本局游戏并输出每局结果
    pub fn complete(&mut self, kick_contestant: bool) -> Result<Vec<RoundResult>> {
        let new_state = match &mut self.state {
            RoomState::Started {
                contestant,
                current_round,
                stage,
                ..
            } if stage.is_end() && *current_round >= self.settings.rounds - 1 => {
                if kick_contestant {
                    RoomState::Created
                } else {
                    RoomState::Joined {
                        contestant: *contestant,
                        ready: false,
                    }
                }
            }
            _ => return Err(Error::InvalidOperation),
        };

        match std::mem::replace(&mut self.state, new_state) {
            RoomState::Started { results, .. } => Ok(results),
            _ => Err(Error::Impossible),
        }
    }
}

/// 一局游戏结果
#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
pub struct GameResult {
    /// 游戏设置
    settings: Settings,
    /// 赢的轮数
    win: u32,
    /// 选择时就选了正确选项
    chosen_win: u32,
    /// 主持人留下的是正确选项
    left_win: u32,
    /// 改变选择的次数
    switch: u32,
    /// 坚持选择的次数
    stick: u32,
    /// 改变选择后赢的次数
    switch_win: u32,
    /// 坚持选择后赢的次数
    stick_win: u32,
}

impl GameResult {
    pub fn calculate<R>(doors: u32, results: R) -> Self
    where
        R: AsRef<[RoundResult]>,
    {
        let results = results.as_ref();
        let settings = Settings::new(doors, results.len() as u32);
        let mut game_result = GameResult {
            settings,
            win: 0,
            chosen_win: 0,
            left_win: 0,
            switch: 0,
            stick: 0,
            switch_win: 0,
            stick_win: 0,
        };

        for result in results {
            if result.chosen == result.prize {
                game_result.chosen_win += 1;
            }

            if result.left == result.prize {
                game_result.left_win += 1;
            }

            match result.decision {
                Decision::Switch => {
                    game_result.switch += 1;
                    if result.win {
                        game_result.win += 1;
                        game_result.switch_win += 1;
                    }
                }
                Decision::Stick => {
                    game_result.stick += 1;
                    if result.win {
                        game_result.win += 1;
                        game_result.stick_win += 1;
                    }
                }
            }
        }

        game_result
    }

    /// 游戏设置
    pub fn settings(&self) -> Settings {
        self.settings
    }

    /// 赢的轮数
    pub fn win(&self) -> u32 {
        self.win
    }

    /// 选择时就选了正确选项
    pub fn chosen_win(&self) -> u32 {
        self.chosen_win
    }

    /// 主持人留下的是正确选项
    pub fn left_win(&self) -> u32 {
        self.left_win
    }

    /// 改变选择的次数
    pub fn switch(&self) -> u32 {
        self.switch
    }

    /// 坚持选择的次数
    pub fn stick(&self) -> u32 {
        self.stick
    }

    /// 改变选择后赢的次数
    pub fn switch_win(&self) -> u32 {
        self.switch_win
    }

    /// 坚持选择后赢的次数
    pub fn stick_win(&self) -> u32 {
        self.stick_win
    }
}

// 在 [0, doors) 范围内生成 exclusive 之外的随机整数
fn random_door(doors: u32, exclusive: u32) -> u32 {
    assert!(
        exclusive < doors,
        "doors = {}, exclusive = {}",
        doors,
        exclusive
    );

    let random = rand::thread_rng().gen_range(0..doors);

    if random >= exclusive {
        random + 1
    } else {
        random
    }
}

#[cfg(test)]
mod test {
    use super::random_door;
    use rand::Rng;

    #[test]
    fn random_door_() {
        let doors = 10;
        for _ in 0..100000 {
            let exclusive = rand::thread_rng().gen_range(0..doors);
            let door = random_door(doors, exclusive);
            assert_ne!(door, exclusive);
        }
    }
}
