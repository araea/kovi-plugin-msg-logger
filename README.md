# kovi-plugin-msg-logger

[<img alt="github" src="https://img.shields.io/badge/github-araea/kovi__plugin__msg__logger-8da0cb?style=for-the-badge&labelColor=555555&logo=github" height="20">](https://github.com/araea/kovi-plugin-msg-logger)
[<img alt="crates.io" src="https://img.shields.io/crates/v/kovi-plugin-msg-logger.svg?style=for-the-badge&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/kovi-plugin-msg-logger)

Kovi 的全量消息记录与分析核心插件。
基于 `SeaORM` + `SQLite` 高性能存储，内置 `Jieba` 中文分词。
**本插件主要专注于数据采集与清洗，并为其他插件提供强大的数据查询 API，本身仅包含基础管理指令。**

## 特性

- 💾 **全量存储** - 完整记录 **接收、发送及多端同步** 的 OneBot 消息（保留原始 JSON、结构化文本及特殊标记）
- 🔄 **多端同步** - 自动记录 Bot 自身发送的消息，以及同一账号在其他客户端（手机/PC）发送的消息，还原完整对话上下文
- 🔍 **中文分词** - 内置 Jieba 分词预处理，自动过滤停用词，建立关键词索引
- 👥 **用户追踪** - 自动记录并更新用户昵称、群名片、活跃时间及统计数据
- 🛡️ **群组管理** - 支持白名单/黑名单模式，灵活控制记录范围
- 🚀 **API 支持** - 为开发者提供词云、热力图、趋势分析等复杂的 SQL 查询接口
- ⚡ **高性能** - 使用 SQLite WAL 模式与异步写入，低资源占用

## 前置

1. 创建 Kovi 项目
2. 执行 `cargo kovi add msg-logger`
3. 在 `src/main.rs` 中添加 `kovi_plugin_msg_logger`

## 快速开始

1. 启动机器人，插件会自动在 `data/kovi-plugin-msg-logger` 初始化数据库。
2. 在 `data/kovi-plugin-msg-logger/config.toml` 中配置管理员或记录模式。
3. 在群内发送 `开启记录`（需管理员权限）开始记录当前群消息。
4. **对于开发者**：在你的插件中调用 `get_logger()` 获取数据进行可视化开发。

## 指令列表

| 指令 | 权限 | 说明 |
|------|------|------|
| `开启记录` | 管理员/群主 | 将当前群加入记录列表（根据黑白名单模式自动调整） |
| `关闭记录` | 管理员/群主 | 停止记录当前群消息 |
| `记录状态` | 所有人 | 查看当前群记录状态及数据库统计概览（消息数/词汇数等） |

> **注意**：本插件不包含生成图片（如词云图）的功能，仅负责记录数据。

## 配置

配置文件路径：`data/kovi-plugin-msg-logger/config.toml`

以下配置与代码默认值保持一致：

```toml
# 记录模式
# "whitelist": 只记录白名单中的群 (默认)
# "blacklist": 记录所有群，除了黑名单中的
mode = "whitelist"

# 是否记录私聊消息
record_private = false

# 管理员列表 (拥有开启/关闭记录的权限)
# 机器人管理员、群主、群管理员默认拥有权限
admins = []

# 分词相关配置
[tokenizer]
# 是否启用分词 (开启后才会生成关键词数据)
enabled = true
# 最小词长度
min_word_length = 2
# 停用词列表 (过滤无意义词汇)
stop_words = [
    "的", "了", "在", "是", "我", "你", "他", "她", "它",
    "有", "和", "与", "这", "那", "就", "也", "都", "而",
    "及", "着", "或", "一个", "没有", "不是", "什么", "怎么",
    "[图片]", "[表情]", "[语音]", "[视频]"
]

[groups]
whitelist = []
blacklist = []
```

## 开发者接口 (Rust)

本插件设计为 Core Library，其他插件可以通过 API 获取清洗后的数据制作高级功能（如：生成今日词云图、年度报告、AI 上下文构建等）。

在你的 `Cargo.toml` 中添加本插件作为依赖，然后在代码中调用：

```rust
use kovi_plugin_msg_logger::get_logger;

// 在你的插件逻辑中
if let Some(logger) = get_logger().await {
    let query = logger.query();
    let group_id = 123456789;
    let user_id = 123456;

    // 1. 获取某群近 7 天的 Top 20 热词 (用于生成词云)
    let words = query.word_cloud(group_id, 20, 7).await?;
    for w in words {
        println!("词: {}, 频次: {}", w.word, w.count);
    }
    
    // 2. 获取某群近 30 天的 24 小时活跃热力图数据
    let heatmap = query.hourly_heatmap(group_id, 30).await?;
    
    // 3. 获取某群特定日期的龙王（发言最多用户）
    let top_talkers = query.top_talkers(group_id, 10, 1).await?;
    
    // 4. 获取星期x小时的二维热力分布 (7x24 grid, 返回 [[i64; 24]; 7])
    let weekly_map = query.weekly_hourly_heatmap(group_id, 90).await?;

    // 5. 获取某用户的个人详细统计（字数、活跃天数、最爱时段等）
    let user_stats = query.user_stats(user_id, Some(group_id)).await?;
    
    // 6. 获取时段对比（例如：本周对比上周消息量变化）
    // 需要传入四个 NaiveDate
    // query.period_comparison(group_id, cur_start, cur_end, prev_start, prev_end).await?;

    // 7. [新] 获取群组最近的上下文消息 (正序，适合构建 LLM 上下文)
    let context_msgs = query.get_recent_group_messages(group_id, 20).await?;

    // 8. [新] 获取指定时间范围的所有消息 (适合日志分析/导出)
    // 传入 Unix 时间戳 (秒)
    let history = query.get_messages_range(start_ts, end_ts).await?;
}
```

### 可用 API 方法概览

**基础统计：**
*   `storage_stats`: 获取数据库总存储统计（消息数、词数、用户数）
*   `message_type_stats`: 获取消息类型分布（纯文/图片/@/回复）

**词频分析：**
*   `word_cloud`: 获取指定天数内的热词
*   `word_cloud_range`: 获取指定日期范围的热词
*   `user_word_cloud`: 获取指定用户的热词

**时间分布：**
*   `hourly_heatmap`: 获取 0-23 点活跃度分布
*   `weekly_hourly_heatmap`: 获取 星期×小时 分布
*   `weekly_distribution`: 获取星期一至星期日的活跃分布
*   `daily_trend`: 获取每日消息量趋势（指定天数）
*   `daily_trend_range`: 获取每日消息量趋势（日期范围）
*   `period_comparison`: 计算两个时间段的消息量变化率

**用户分析：**
*   `top_talkers`: 获取活跃用户排行（龙王榜）
*   `top_talkers_range`: 指定日期范围的活跃排行
*   `user_stats`: 获取单用户深度分析（包含排名、最爱时段、平均字数等）
*   `user_group_activity`: 获取用户在所有群的活跃度分布

**检索与上下文：**
*   `get_recent_group_messages`: 获取群组最近消息上下文（正序，Limit限制）
*   `get_messages_by_time_range`: 获取指定时间戳范围内的完整消息日志
*   `search_messages`: 全文搜索消息
*   `user_messages`: 获取指定用户的历史消息列表

## 技术栈

- **ORM**: [SeaORM](https://www.sea-ql.org/SeaORM/)
- **Database**: SQLite (WAL Mode)
- **Segmentation**: [Jieba-rs](https://github.com/messense/jieba-rs)

## 致谢

- [Kovi](https://kovi.threkork.com/)

<br>

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
