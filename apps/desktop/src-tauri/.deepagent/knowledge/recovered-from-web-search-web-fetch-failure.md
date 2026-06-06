---
title: Recovered from web_search, web_fetch failure
kind: pitfall
tags: [auto-captured, failure-recovery, web-search, web-fetch]
created_at: 1780654209977
updated_at: 1780654209977
source_session: ses_019e97426fae7901b1bcc51fa516a3e0
---

## Symptom
A previous run hit failed tool calls while handling this request:

今天长沙的天气怎么样，有没有什么热搜？

## Failed tools
web_search, web_fetch

## What worked
The run recovered and produced a final answer. On a similar failure, do not stop at the first failed tool call; inspect the error, try an alternate query/source/tool, and preserve the workaround for reuse.

For `web_search` failures such as unparseable results or provider markup changes, retry with a different query/source or use `web_fetch` against a known authoritative URL.

## Recovered answer
## ☁️ 长沙天气（2026年6月5日 星期五）

| 时段 | 天气 | 气温 | 体感 |
|------|------|------|------|
| **当前** | 零星阵雨🌦️ | 32°C | 湿度56%，微风~4km/h |
| 早上 | 零星小雨 | 26°C | 29°C |
| 中午 | 零星阵雨 | 28°C | 32°C |
| 傍晚 | 小阵雨 | 26°C | 30°C |
| 夜间 | 零星阵雨 | 24°C | 27°C |

**总结：** 今天长沙有分散性阵雨，气温 24~32°C，湿度较高（~70%），体感偏闷热。出门建议带伞。

---

## 🔥 今日热搜 / 热点要闻（来自百度新闻）

1. **习近平为老挝领导人通伦举行欢迎仪式**
2. **中方回应"黄岩岛不明结构物消失"**
3. **国务院督导查处双汇猪肉抗生素超标问题**
4. **用"平常心"为高考"助攻"**（明天6月7日高考）
5. **日本拥核已是现实威胁**
6. **二手油车价格崩盘**
7. **网民谎称售卖2026高考题诈骗被查**
8. **英国间谍2次偷窃中国茶叶机密**
9. **曾经的"国民饮料"康师傅卖不动了**
10. **豆包误判蘑菇致用户中毒，高管回应**
11. **受贿1.58亿余元，齐扎拉被判无期**
12. **39岁男子海口海边散步落水，漂流7天6夜后被救起**
13. **1.08米袖珍男孩专升本上岸，在深圳闯出名堂**

## Evidence
## User goal
今天长沙的天气怎么样，有没有什么热搜？

## Failures encountered
- web_search failed: search failed: search returned no parseable results (the provider may have changed its markup)
- web_search failed: search failed: search returned no parseable results (the provider may have changed its markup)
- web_fetch failed: fetch failed: GET https://www.zhihu.com/hot returned 403 Forbidden
- web_search failed: search failed: search returned no parseable results (the provider may have changed its markup)
- web_fetch failed: fetch failed: GET https://tophub.today/c/weibo returned 404 Not Found
- web_search failed: search failed: search returned no parseable results (the provider may have changed its markup)

## Final resolution
## ☁️ 长沙天气（2026年6月5日 星期五）

| 时段 | 天气 | 气温 | 体感 |
|------|------|------|------|
| **当前** | 零星阵雨🌦️ | 32°C | 湿度56%，微风~4km/h |
| 早上 | 零星小雨 | 26°C | 29°C |
| 中午 | 零星阵雨 | 28°C | 32°C |
| 傍晚 | 小阵雨 | 26°C | 30°C |
| 夜间 | 零星阵雨 | 24°C | 27°C |

**总结：** 今天长沙有分散性阵雨，气温 24~32°C，湿度较高（~70%），体感偏闷热。出门建议带伞。

---

## 🔥 今日热搜 / 热点要闻（来自百度新闻）

1. **习近平为老挝领导人通伦举行欢迎仪式**
2. **中方回应"黄岩岛不明结构物消失"**
3. **国务院督导查处双汇猪肉抗生素超标问题**
4. **用"平常心"为高考"助攻"**（明天6月7日高考）
5. **日本拥核已是现实威胁**
6. **二手油车价格崩盘**
7. **网民谎称售卖2026高考题诈骗被查**
8. **英国间谍2次偷窃中国茶叶机密**
9. **曾经的"国民饮料"康师傅卖不动了**
10. **豆包误判蘑菇致用户中毒，高管回应**
11. **受贿1.58亿余元，齐扎拉被判无期**
12. **39岁男子海口海边散步落水，漂流7天6夜后被救起**
13. **1.08米袖珍男孩专升本上岸，在深圳闯出名堂**
