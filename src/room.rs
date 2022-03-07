#![allow(dead_code)]
use crate::error::*;
use rand::Rng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
        win: u32,

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

/// 游戏房间
#[derive(Debug, Serialize, Deserialize)]
pub struct Room {
    /// 房间 ID
    id: Uuid,
    /// 主持人 ID
    host: Uuid,
    settings: Settings,
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

    /// 当前游戏配置
    pub fn settings(&self) -> Settings {
        self.settings
    }

    /// 更新设置，返回 `true` 表示需要通知挑战者重新选择就绪
    pub fn update_settings(&mut self, settings: Settings) -> Result<bool> {
        match &mut self.state {
            RoomState::Created => {
                self.settings = settings;
                Ok(false)
            }
            RoomState::Joined { ready, .. } => {
                let notify_contestant = *ready;
                self.settings = settings;
                // 如果挑战者已经就绪，需要重置，让挑战者重新选择就绪
                *ready = false;
                Ok(notify_contestant)
            }
            RoomState::Started { .. } => Err(Error::InvalidOperation),
        }
    }

    /// 开始游戏并将奖品随机放到一个门内
    pub fn start_random(&mut self) -> Result<u32> {
        match self.state {
            RoomState::Joined { ready, contestant } if ready => {
                let prize = rand::thread_rng().gen_range(0..self.settings.doors);
                self.state = RoomState::Started {
                    contestant,
                    current_round: 0,
                    prize,
                    win: 0,
                    stage: Stage::Choose,
                };
                Ok(prize)
            }
            RoomState::Started {
                contestant,
                current_round,
                win,
                stage: Stage::End { .. },
                ..
            } if current_round < self.settings.rounds - 1 => {
                let prize = rand::thread_rng().gen_range(0..self.settings.doors);
                self.state = RoomState::Started {
                    contestant,
                    current_round: current_round + 1,
                    prize,
                    win,
                    stage: Stage::Choose,
                };
                Ok(prize)
            }
            _ => Err(Error::InvalidOperation),
        }
    }

    /// 开始游戏并将奖品放到序号指定的门内
    pub fn start(&mut self, prize: u32) -> Result<()> {
        if prize >= self.settings.doors {
            return Err(Error::InvalidDoorIndex);
        }
        match self.state {
            RoomState::Joined { ready, contestant } if ready => {
                self.state = RoomState::Started {
                    contestant,
                    current_round: 0,
                    prize,
                    win: 0,
                    stage: Stage::Choose,
                };
                Ok(())
            }
            RoomState::Started {
                contestant,
                current_round,
                prize,
                win,
                stage: Stage::End { .. },
            } if current_round < self.settings.rounds - 1 => {
                self.state = RoomState::Started {
                    contestant,
                    current_round: current_round + 1,
                    prize,
                    win,
                    stage: Stage::Choose,
                };
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
            contestant,
            current_round,
            prize,
            win,
            stage: Stage::Decide { chosen, left },
        } = self.state
        {
            let win_the_prize = match (chosen, left, decision) {
                (p, _, Decision::Stick) | (_, p, Decision::Switch) if p == prize => true,
                _ => false,
            };
            let result = RoundResult {
                prize,
                chosen,
                left,
                decision,
                win: win_the_prize,
            };
            self.state = RoomState::Started {
                contestant,
                current_round,
                prize,
                win: if win_the_prize { win + 1 } else { win },
                stage: Stage::End { result },
            };
            Ok(result)
        } else {
            Err(Error::InvalidOperation)
        }
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
