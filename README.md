kovi-plugin-msg-logger
======================

[<img alt="github" src="https://img.shields.io/badge/github-araea/kovi__plugin__msg__logger-8da0cb?style=for-the-badge&labelColor=555555&logo=github" height="20">](https://github.com/araea/kovi-plugin-msg-logger)
[<img alt="crates.io" src="https://img.shields.io/crates/v/kovi-plugin-msg-logger.svg?style=for-the-badge&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/kovi-plugin-msg-logger)

Kovi 的全量消息记录与分析插件。基于 `SeaORM` + `SQLite` 高性能存储，内置 `Jieba` 中文分词，为数据可视化提供强大的后端支持。

## 特性

- 💾 **全量存储** - 完整记录 OneBot 消息字段（原始 JSON、结构化数据）
- 🔍 **中文分词** - 内置 Jieba 分词预处理，支持自定义停用词过滤
- 📊 **数据分析** - 提供词云、热力图、龙王榜等可视化查询接口
- 👥 **用户追踪** - 自动记录用户昵称变更、活跃时间与群名片
- 🛡️ **群组管理** - 支持白名单/黑名单模式，灵活控制记录范围
- 🚀 **高性能** - 使用 SQLite WAL 模式与异步写入，低资源占用

## 前置

1. 创建 Kovi 项目
2. 执行 `cargo kovi add msg-logger`
3. 在 `src/main.rs` 中添加 `kovi_plugin_msg_logger`

## 快速开始

1. 启动机器人，插件会自动在 `data/kovi-plugin-msg-logger` 初始化数据库。
2. 在群内发送 `开启记录` 开始记录当前群消息（取决于配置模式）。
3. 积累一定数据后，发送 `本群词云` 查看效果。

## 指令列表

| 指令 | 说明 |
|------|------|
| `开启记录` | 将当前群加入记录列表 |
| `关闭记录` | 停止记录当前群消息 |
| `记录状态` | 查看当前群记录状态及数据库统计概览 |
| `本群词云` | 生成本群近 7 天的 Top 20 热词 |
| `本群热力图` | 生成本群近 30 天的 24 小时活跃分布 |
| `龙王榜` | 查看本群近 7 天的发言 Top 10 用户 |

## 配置

资源目录：`data/kovi-plugin-msg-logger/config.toml`

```toml
# 记录模式
# "whitelist": 只记录白名单中的群 (默认)
# "blacklist": 记录所有群，除了黑名单中的
mode = "whitelist"

# 是否记录私聊消息
record_private = false

# 分词相关配置
[tokenizer]
# 是否启用分词 (建议开启以支持词云)
enabled = true
# 最小词长度
min_word_length = 2
# 停用词列表 (过滤无意义词汇)
stop_words = ["的", "了", "在", "是", ...]

[groups]
whitelist = []
blacklist = []
```

## 开发者接口

本插件设计为核心库，其他插件可以通过 API 获取清洗后的数据进行高级可视化（如生成图片）。

```rust
// 在其他插件中调用
if let Some(logger) = kovi_plugin_msg_logger::get_logger().await {
    // 获取某群词云数据
    let words = logger.query().word_cloud(group_id, 20, 7).await?;
    
    // 获取某群活跃热力图
    let heatmap = logger.query().hourly_heatmap(group_id, 30).await?;
    
    // 获取全库统计
    let stats = logger.query().storage_stats().await;
}
```

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
