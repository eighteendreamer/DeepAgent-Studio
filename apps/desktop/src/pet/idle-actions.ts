import type { Character } from "./api";

export const RUNNING_ACTION = "原地左转奔跑";

const idlePool = [
  { name: "东张西望", weight: 3 },
  { name: "悠闲哼歌", weight: 3 },
  { name: "超大伸懒腰", weight: 3 },
  { name: "哈欠连天", weight: 3 },
  { name: "原地小憩沉眠", weight: 1 },
  { name: RUNNING_ACTION, weight: 1 },
  { name: "吃白饭", weight: 1 },
  { name: "大口吃零食", weight: 1 },
  { name: "原地专心玩魔方", weight: 1 },
  { name: "原地蹲下玩玩具汽车", weight: 1 },
  { name: "鲸鱼吐泡泡特效", weight: 1 },
  { name: "撸猫", weight: 1 },
] as const;

export const idleDelay = (random = Math.random()) => Math.round(45_000 + random * 45_000);

export function chooseIdleAction(character: Character, previous?: string, random = Math.random()) {
  const available = idlePool.filter(({ name }) => character.actions[name]);
  const different = available.filter(({ name }) => name !== previous);
  const pool = different.length ? different : available;
  let pick = random * pool.reduce((sum, { weight }) => sum + weight, 0);
  for (const { name, weight } of pool) {
    pick -= weight;
    if (pick < 0) return name;
  }
  return pool[pool.length - 1]?.name;
}
