//! # kovi-plugin-msg-logger
//!
//! 全量消息记录器，支持 Jieba 分词预处理，为数据可视化提供高性能后端。
//!
//! ## 功能特性
//! - 完整的 OneBot 消息存储（保留原始 JSON 和结构化字段）
//! - Jieba 中文分词预处理，支持自定义停用词
//! - 用户信息表，自动追踪昵称/群名片变化
//! - 丰富的可视化数据查询 API
//! - 按群组配置记录策略（白名单/黑名单模式）
//!
//! ## 对外 API
//! ```ignore
//! // 在其他插件中获取 Logger 实例
//! if let Some(logger) = kovi_plugin_msg_logger::get_logger().await {
//!     // 词云数据
//!     let words = logger.query().word_cloud(group_id, 20, 7).await?;
//!     // 活跃热力图
//!     let heatmap = logger.query().hourly_heatmap(group_id, 30).await?;
//!     // 用户排行
//!     let talkers = logger.query().top_talkers(group_id, 10, 7).await?;
//! }
//! ```

// =============================
//          Modules
// =============================

/// 数据库实体定义
pub mod entities {
    pub mod prelude {
        pub use super::keywords::Entity as Keywords;
        pub use super::messages::Entity as Messages;
        pub use super::users::Entity as Users;
    }

    /// 消息表：存储完整的消息数据
    pub mod messages {
        use sea_orm::entity::prelude::*;
        use serde::{Deserialize, Serialize};

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "messages")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i64,
            /// OneBot 消息 ID
            pub message_id: i64,
            /// 发送者 QQ
            pub user_id: i64,
            /// 群号（私聊为 NULL）
            pub group_id: Option<i64>,
            /// 消息类型：group / private
            pub msg_type: String,
            /// 消息子类型：normal / anonymous / notice 等
            pub sub_type: Option<String>,
            /// 完整原始 JSON 数据
            #[sea_orm(column_type = "Text")]
            pub raw_json: String,
            /// 清洗后的纯文本
            #[sea_orm(column_type = "Text")]
            pub clean_text: String,
            /// 消息长度（字符数）
            pub text_length: i32,
            /// 是否包含图片
            pub has_image: bool,
            /// 是否包含 @
            pub has_at: bool,
            /// 是否为回复消息
            pub is_reply: bool,
            /// 发送时昵称快照
            pub sender_nickname: String,
            /// 发送时群名片快照
            pub sender_card: Option<String>,
            /// 发送时群角色：owner / admin / member
            pub sender_role: Option<String>,
            /// Unix 时间戳
            pub created_at: i64,
            /// 小时（0-23），冗余存储便于统计
            pub hour_of_day: i32,
            /// 星期几（0=周日, 1-6=周一至周六）
            pub day_of_week: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {
            #[sea_orm(has_many = "super::keywords::Entity")]
            Keywords,
            #[sea_orm(
                belongs_to = "super::users::Entity",
                from = "Column::UserId",
                to = "super::users::Column::UserId"
            )]
            User,
        }

        impl Related<super::keywords::Entity> for Entity {
            fn to() -> RelationDef {
                Relation::Keywords.def()
            }
        }

        impl Related<super::users::Entity> for Entity {
            fn to() -> RelationDef {
                Relation::User.def()
            }
        }

        impl ActiveModelBehavior for ActiveModel {}
    }

    /// 关键词表：存储分词结果
    pub mod keywords {
        use sea_orm::entity::prelude::*;
        use serde::{Deserialize, Serialize};

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "keywords")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i64,
            /// 外键关联 messages.id
            pub message_id: i64,
            /// 分词结果
            pub word: String,
            /// 词长度，便于过滤
            pub word_length: i32,
            /// 群号（冗余存储方便统计）
            pub group_id: Option<i64>,
            /// 用户 ID（冗余存储方便统计）
            pub user_id: i64,
            /// Unix 时间戳
            pub created_at: i64,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {
            #[sea_orm(
                belongs_to = "super::messages::Entity",
                from = "Column::MessageId",
                to = "super::messages::Column::Id"
            )]
            Message,
        }

        impl Related<super::messages::Entity> for Entity {
            fn to() -> RelationDef {
                Relation::Message.def()
            }
        }

        impl ActiveModelBehavior for ActiveModel {}
    }

    /// 用户表：追踪用户信息变化
    pub mod users {
        use sea_orm::entity::prelude::*;
        use serde::{Deserialize, Serialize};

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "users")]
        pub struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub user_id: i64,
            /// 最新昵称
            pub nickname: String,
            /// 首次记录时间
            pub first_seen: i64,
            /// 最后活跃时间
            pub last_seen: i64,
            /// 总消息数
            pub message_count: i64,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {
            #[sea_orm(has_many = "super::messages::Entity")]
            Messages,
        }

        impl Related<super::messages::Entity> for Entity {
            fn to() -> RelationDef {
                Relation::Messages.def()
            }
        }

        impl ActiveModelBehavior for ActiveModel {}
    }
}

/// 配置管理
pub mod config {
    use kovi::toml;
    use kovi::utils::{load_toml_data, save_toml_data};
    use parking_lot::RwLock;
    use serde::{Deserialize, Serialize};
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::Arc;

    pub static CONFIG: std::sync::OnceLock<Arc<RwLock<Config>>> = std::sync::OnceLock::new();

    pub fn get() -> Arc<RwLock<Config>> {
        CONFIG.get().cloned().expect("Config not initialized")
    }

    const DEFAULT_CONFIG: &str = r#"
# 记录模式
# "whitelist": 只记录白名单中的群
# "blacklist": 记录所有群，除了黑名单中的
mode = "whitelist"

# 是否记录私聊消息
record_private = false

# 分词相关配置
[tokenizer]
# 是否启用分词
enabled = true
# 最小词长度（字符数）
min_word_length = 2
# 停用词列表
stop_words = [
    "的", "了", "在", "是", "我", "你", "他", "她", "它",
    "有", "和", "与", "这", "那", "就", "也", "都", "而",
    "及", "着", "或", "一个", "没有", "不是", "什么", "怎么",
    "[图片]", "[表情]", "[语音]", "[视频]"
]

[groups]
whitelist = []
blacklist = []
"#;

    #[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
    #[serde(rename_all = "lowercase")]
    pub enum RecordMode {
        Blacklist,
        Whitelist,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct GroupLists {
        pub whitelist: Vec<i64>,
        pub blacklist: Vec<i64>,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct TokenizerConfig {
        pub enabled: bool,
        pub min_word_length: usize,
        pub stop_words: Vec<String>,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct Config {
        pub mode: RecordMode,
        pub record_private: bool,
        pub tokenizer: TokenizerConfig,
        pub groups: GroupLists,

        #[serde(skip)]
        config_path: PathBuf,
        #[serde(skip)]
        stop_words_set: HashSet<String>,
    }

    impl Config {
        pub fn load(data_dir: PathBuf) -> Arc<RwLock<Self>> {
            if !data_dir.exists() {
                std::fs::create_dir_all(&data_dir).expect("Failed to create data dir");
            }
            let config_path = data_dir.join("config.toml");

            let default: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
            let mut config =
                load_toml_data(default.clone(), config_path.clone()).unwrap_or(default);

            config.config_path = config_path;
            config.rebuild_stop_words_set();

            Arc::new(RwLock::new(config))
        }

        fn rebuild_stop_words_set(&mut self) {
            self.stop_words_set = self.tokenizer.stop_words.iter().cloned().collect();
        }

        pub fn save(&self) {
            let _ = save_toml_data(self, &self.config_path);
        }

        pub fn is_stop_word(&self, word: &str) -> bool {
            self.stop_words_set.contains(word)
        }

        pub fn should_record_group(&self, group_id: i64) -> bool {
            match self.mode {
                RecordMode::Whitelist => self.groups.whitelist.contains(&group_id),
                RecordMode::Blacklist => !self.groups.blacklist.contains(&group_id),
            }
        }

        pub fn should_record_private(&self) -> bool {
            self.record_private
        }

        /// 开启群记录，返回操作结果消息
        pub fn enable_group(&mut self, group_id: i64) -> &'static str {
            match self.mode {
                RecordMode::Whitelist => {
                    if !self.groups.whitelist.contains(&group_id) {
                        self.groups.whitelist.push(group_id);
                        self.save();
                        "✅ 已开启本群消息记录"
                    } else {
                        "⚠️ 本群记录已处于开启状态"
                    }
                }
                RecordMode::Blacklist => {
                    if let Some(pos) = self.groups.blacklist.iter().position(|&x| x == group_id) {
                        self.groups.blacklist.remove(pos);
                        self.save();
                        "✅ 已开启本群消息记录"
                    } else {
                        "⚠️ 本群记录已处于开启状态"
                    }
                }
            }
        }

        /// 关闭群记录，返回操作结果消息
        pub fn disable_group(&mut self, group_id: i64) -> &'static str {
            match self.mode {
                RecordMode::Whitelist => {
                    if let Some(pos) = self.groups.whitelist.iter().position(|&x| x == group_id) {
                        self.groups.whitelist.remove(pos);
                        self.save();
                        "🛑 已关闭本群消息记录"
                    } else {
                        "⚠️ 本群记录已处于关闭状态"
                    }
                }
                RecordMode::Blacklist => {
                    if !self.groups.blacklist.contains(&group_id) {
                        self.groups.blacklist.push(group_id);
                        self.save();
                        "🛑 已关闭本群消息记录"
                    } else {
                        "⚠️ 本群记录已处于关闭状态"
                    }
                }
            }
        }
    }
}

/// 数据库管理与查询层
pub mod db {
    use super::config;
    use super::entities::{prelude::*, *};
    use jieba_rs::Jieba;
    use kovi::chrono::{Datelike, TimeZone, Timelike};
    use kovi::MsgEvent;
    use sea_orm::{
        ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, Database, DatabaseConnection,
        DbBackend, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Schema,
        Statement,
    };
    use std::path::PathBuf;
    use std::sync::Arc;

    /// 分词配置快照，避免长时间持有配置锁
    #[derive(Clone)]
    struct TokenizerSnapshot {
        enabled: bool,
        min_word_length: usize,
        stop_words: std::collections::HashSet<String>,
    }

    impl TokenizerSnapshot {
        fn from_config(cfg: &config::Config) -> Self {
            Self {
                enabled: cfg.tokenizer.enabled,
                min_word_length: cfg.tokenizer.min_word_length,
                stop_words: cfg.tokenizer.stop_words.iter().cloned().collect(),
            }
        }

        fn is_stop_word(&self, word: &str) -> bool {
            self.stop_words.contains(word)
        }
    }

    /// 消息记录器核心结构
    pub struct Logger {
        db: DatabaseConnection,
        jieba: Arc<Jieba>,
        query_api: QueryApi,
    }

    impl Logger {
        pub async fn new(data_dir: PathBuf) -> Self {
            if !data_dir.exists() {
                std::fs::create_dir_all(&data_dir).unwrap();
            }
            let db_path = data_dir.join("msg_history.sqlite");
            let db_url = format!("sqlite://{}?mode=rwc", db_path.to_string_lossy());

            let db = Database::connect(&db_url)
                .await
                .expect("Failed to connect to SQLite");

            Self::init_database(&db).await;

            // Jieba 初始化是阻塞操作，在 blocking 线程中执行
            let jieba = tokio::task::spawn_blocking(Jieba::new)
                .await
                .expect("Failed to initialize Jieba");

            let query_api = QueryApi { db: db.clone() };

            Self {
                db,
                jieba: Arc::new(jieba),
                query_api,
            }
        }

        async fn init_database(db: &DatabaseConnection) {
            let builder = db.get_database_backend();
            let schema = Schema::new(builder);

            // 创建表
            let _ = db
                .execute(builder.build(schema.create_table_from_entity(Messages).if_not_exists()))
                .await;
            let _ = db
                .execute(builder.build(schema.create_table_from_entity(Keywords).if_not_exists()))
                .await;
            let _ = db
                .execute(builder.build(schema.create_table_from_entity(Users).if_not_exists()))
                .await;

            // 创建索引以加速查询
            let indexes = [
                "CREATE INDEX IF NOT EXISTS idx_messages_group_id ON messages(group_id)",
                "CREATE INDEX IF NOT EXISTS idx_messages_user_id ON messages(user_id)",
                "CREATE INDEX IF NOT EXISTS idx_messages_created_at ON messages(created_at)",
                "CREATE INDEX IF NOT EXISTS idx_messages_group_time ON messages(group_id, created_at)",
                "CREATE INDEX IF NOT EXISTS idx_keywords_word ON keywords(word)",
                "CREATE INDEX IF NOT EXISTS idx_keywords_group_id ON keywords(group_id)",
                "CREATE INDEX IF NOT EXISTS idx_keywords_group_time ON keywords(group_id, created_at)",
                "PRAGMA journal_mode=WAL",
                "PRAGMA synchronous=NORMAL",
                "PRAGMA cache_size=10000",
            ];

            for sql in indexes {
                let _ = db
                    .execute(Statement::from_string(DbBackend::Sqlite, sql))
                    .await;
            }
        }

        /// 获取查询 API
        pub fn query(&self) -> &QueryApi {
            &self.query_api
        }

        /// 记录消息（核心方法）
        pub async fn log_message(&self, event: &Arc<MsgEvent>) -> anyhow::Result<()> {
            // 使用事件时间戳来计算时间字段，保持一致性
            let created_at = event.time;
            let datetime = kovi::chrono::Local
                .timestamp_opt(created_at, 0)
                .single()
                .unwrap_or_else(kovi::chrono::Local::now);
            let hour_of_day = datetime.hour() as i32;
            let day_of_week = datetime.weekday().num_days_from_sunday() as i32;

            // 提取消息文本和元数据
            let msg_text = event.borrow_text().unwrap_or("").to_string();
            let raw_json = event.original_json.to_string();

            // 检测消息特征
            let has_image = raw_json.contains("\"type\":\"image\"");
            let has_at = raw_json.contains("\"type\":\"at\"");
            let is_reply = raw_json.contains("\"type\":\"reply\"");

            // 插入消息记录
            let msg_model = messages::ActiveModel {
                message_id: ActiveValue::Set(event.message_id as i64),
                user_id: ActiveValue::Set(event.user_id),
                group_id: ActiveValue::Set(event.group_id),
                msg_type: ActiveValue::Set(event.message_type.clone()),
                sub_type: ActiveValue::Set(Some(event.sub_type.clone())),
                raw_json: ActiveValue::Set(raw_json),
                clean_text: ActiveValue::Set(msg_text.clone()),
                text_length: ActiveValue::Set(msg_text.chars().count() as i32),
                has_image: ActiveValue::Set(has_image),
                has_at: ActiveValue::Set(has_at),
                is_reply: ActiveValue::Set(is_reply),
                sender_nickname: ActiveValue::Set(
                    event.sender.nickname.clone().unwrap_or_default(),
                ),
                sender_card: ActiveValue::Set(event.sender.card.clone()),
                sender_role: ActiveValue::Set(event.sender.role.clone()),
                created_at: ActiveValue::Set(created_at),
                hour_of_day: ActiveValue::Set(hour_of_day),
                day_of_week: ActiveValue::Set(day_of_week),
                ..Default::default()
            };

            let inserted = msg_model.insert(&self.db).await?;
            let db_id = inserted.id;

            // 更新用户表
            self.upsert_user(event, created_at).await?;

            // 快速获取分词配置快照，立即释放锁
            let tokenizer_snapshot = {
                let cfg = config::get();
                let cfg_read = cfg.read();
                TokenizerSnapshot::from_config(&cfg_read)
            };

            // 分词处理（在 blocking 线程中执行，避免阻塞异步运行时）
            if tokenizer_snapshot.enabled && !msg_text.trim().is_empty() {
                let jieba = self.jieba.clone();
                let group_id = event.group_id;
                let user_id = event.user_id;

                // 在 blocking 线程中执行分词
                let keywords_data = tokio::task::spawn_blocking(move || {
                    let words = jieba.cut(&msg_text, true);
                    let min_len = tokenizer_snapshot.min_word_length;

                    words
                        .into_iter()
                        .filter(|w| {
                            let s = w.trim();
                            let len = s.chars().count();
                            len >= min_len && !tokenizer_snapshot.is_stop_word(s)
                        })
                        .map(|w| (w.to_string(), w.chars().count() as i32))
                        .collect::<Vec<_>>()
                })
                .await?;

                if !keywords_data.is_empty() {
                    let keywords: Vec<keywords::ActiveModel> = keywords_data
                        .into_iter()
                        .map(|(word, word_length)| keywords::ActiveModel {
                            message_id: ActiveValue::Set(db_id),
                            word: ActiveValue::Set(word),
                            word_length: ActiveValue::Set(word_length),
                            group_id: ActiveValue::Set(group_id),
                            user_id: ActiveValue::Set(user_id),
                            created_at: ActiveValue::Set(created_at),
                            ..Default::default()
                        })
                        .collect();

                    keywords::Entity::insert_many(keywords)
                        .exec(&self.db)
                        .await?;
                }
            }

            Ok(())
        }

        async fn upsert_user(&self, event: &Arc<MsgEvent>, timestamp: i64) -> anyhow::Result<()> {
            let nickname = event.sender.nickname.clone().unwrap_or_default();

            // 尝试查找现有用户
            let existing = Users::find_by_id(event.user_id).one(&self.db).await?;

            match existing {
                Some(user) => {
                    // 更新现有用户
                    let new_count = user.message_count + 1;
                    let mut active: users::ActiveModel = user.into();
                    active.nickname = ActiveValue::Set(nickname);
                    active.last_seen = ActiveValue::Set(timestamp);
                    active.message_count = ActiveValue::Set(new_count);
                    active.update(&self.db).await?;
                }
                None => {
                    // 创建新用户
                    let new_user = users::ActiveModel {
                        user_id: ActiveValue::Set(event.user_id),
                        nickname: ActiveValue::Set(nickname),
                        first_seen: ActiveValue::Set(timestamp),
                        last_seen: ActiveValue::Set(timestamp),
                        message_count: ActiveValue::Set(1),
                    };
                    new_user.insert(&self.db).await?;
                }
            }

            Ok(())
        }
    }

    /// 查询 API - 为可视化插件提供数据接口
    #[derive(Clone)]
    pub struct QueryApi {
        db: DatabaseConnection,
    }

    /// 词频统计结果
    #[derive(Debug, Clone)]
    pub struct WordCount {
        pub word: String,
        pub count: i64,
    }

    /// 用户活跃统计
    #[derive(Debug, Clone)]
    pub struct UserActivity {
        pub user_id: i64,
        pub nickname: String,
        pub message_count: i64,
    }

    /// 时段统计
    #[derive(Debug, Clone)]
    pub struct HourlyStats {
        pub hour: i32,
        pub count: i64,
    }

    /// 每日统计
    #[derive(Debug, Clone)]
    pub struct DailyStats {
        pub date: String,
        pub count: i64,
    }

    /// 存储统计
    #[derive(Debug, Clone)]
    pub struct StorageStats {
        pub total_messages: u64,
        pub total_keywords: u64,
        pub total_users: u64,
        pub groups_tracked: u64,
    }

    impl QueryApi {
        /// 获取词云数据（Top N 高频词）
        pub async fn word_cloud(
            &self,
            group_id: i64,
            limit: u64,
            days: i64,
        ) -> anyhow::Result<Vec<WordCount>> {
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            // 使用参数化查询防止 SQL 注入
            let sql = format!(
                "SELECT word, COUNT(*) as count FROM keywords \
                 WHERE group_id = {} AND created_at >= {} \
                 GROUP BY word ORDER BY count DESC LIMIT {}",
                group_id, start_time, limit
            );

            let rows = self
                .db
                .query_all(Statement::from_string(DbBackend::Sqlite, sql))
                .await?;

            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                result.push(WordCount {
                    word: row.try_get("", "word")?,
                    count: row.try_get("", "count")?,
                });
            }
            Ok(result)
        }

        /// 获取用户专属词云
        pub async fn user_word_cloud(
            &self,
            user_id: i64,
            group_id: Option<i64>,
            limit: u64,
            days: i64,
        ) -> anyhow::Result<Vec<WordCount>> {
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let group_filter = match group_id {
                Some(gid) => format!("AND group_id = {}", gid),
                None => String::new(),
            };

            let sql = format!(
                "SELECT word, COUNT(*) as count FROM keywords \
                 WHERE user_id = {} AND created_at >= {} {} \
                 GROUP BY word ORDER BY count DESC LIMIT {}",
                user_id, start_time, group_filter, limit
            );

            let rows = self
                .db
                .query_all(Statement::from_string(DbBackend::Sqlite, sql))
                .await?;

            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                result.push(WordCount {
                    word: row.try_get("", "word")?,
                    count: row.try_get("", "count")?,
                });
            }
            Ok(result)
        }

        /// 获取24小时活跃热力图
        pub async fn hourly_heatmap(
            &self,
            group_id: i64,
            days: i64,
        ) -> anyhow::Result<Vec<HourlyStats>> {
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT hour_of_day as hour, COUNT(*) as count FROM messages \
                 WHERE group_id = {} AND created_at >= {} \
                 GROUP BY hour_of_day ORDER BY hour_of_day",
                group_id, start_time
            );

            let rows = self
                .db
                .query_all(Statement::from_string(DbBackend::Sqlite, sql))
                .await?;

            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                result.push(HourlyStats {
                    hour: row.try_get("", "hour")?,
                    count: row.try_get("", "count")?,
                });
            }
            Ok(result)
        }

        /// 获取星期活跃分布
        pub async fn weekly_distribution(
            &self,
            group_id: i64,
            days: i64,
        ) -> anyhow::Result<Vec<(i32, i64)>> {
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT day_of_week, COUNT(*) as count FROM messages \
                 WHERE group_id = {} AND created_at >= {} \
                 GROUP BY day_of_week ORDER BY day_of_week",
                group_id, start_time
            );

            let rows = self
                .db
                .query_all(Statement::from_string(DbBackend::Sqlite, sql))
                .await?;

            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                let dow: i32 = row.try_get("", "day_of_week")?;
                let count: i64 = row.try_get("", "count")?;
                result.push((dow, count));
            }
            Ok(result)
        }

        /// 获取每日消息趋势
        pub async fn daily_trend(
            &self,
            group_id: i64,
            days: i64,
        ) -> anyhow::Result<Vec<DailyStats>> {
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT date(created_at, 'unixepoch', 'localtime') as date, COUNT(*) as count \
                 FROM messages WHERE group_id = {} AND created_at >= {} \
                 GROUP BY date ORDER BY date",
                group_id, start_time
            );

            let rows = self
                .db
                .query_all(Statement::from_string(DbBackend::Sqlite, sql))
                .await?;

            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                result.push(DailyStats {
                    date: row.try_get("", "date")?,
                    count: row.try_get("", "count")?,
                });
            }
            Ok(result)
        }

        /// 获取活跃用户排行（龙王榜）
        pub async fn top_talkers(
            &self,
            group_id: i64,
            limit: u64,
            days: i64,
        ) -> anyhow::Result<Vec<UserActivity>> {
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT m.user_id, COALESCE(u.nickname, '') as nickname, COUNT(*) as count \
                 FROM messages m \
                 LEFT JOIN users u ON m.user_id = u.user_id \
                 WHERE m.group_id = {} AND m.created_at >= {} \
                 GROUP BY m.user_id ORDER BY count DESC LIMIT {}",
                group_id, start_time, limit
            );

            let rows = self
                .db
                .query_all(Statement::from_string(DbBackend::Sqlite, sql))
                .await?;

            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                result.push(UserActivity {
                    user_id: row.try_get("", "user_id")?,
                    nickname: row.try_get::<String>("", "nickname").unwrap_or_default(),
                    message_count: row.try_get("", "count")?,
                });
            }
            Ok(result)
        }

        /// 获取用户在各群的活跃度
        pub async fn user_group_activity(&self, user_id: i64) -> anyhow::Result<Vec<(i64, i64)>> {
            let sql = format!(
                "SELECT group_id, COUNT(*) as count FROM messages \
                 WHERE user_id = {} AND group_id IS NOT NULL \
                 GROUP BY group_id ORDER BY count DESC",
                user_id
            );

            let rows = self
                .db
                .query_all(Statement::from_string(DbBackend::Sqlite, sql))
                .await?;

            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                let gid: i64 = row.try_get("", "group_id")?;
                let count: i64 = row.try_get("", "count")?;
                result.push((gid, count));
            }
            Ok(result)
        }

        /// 获取存储统计概况
        pub async fn storage_stats(&self) -> StorageStats {
            let msg_count = Messages::find().count(&self.db).await.unwrap_or(0);
            let kw_count = Keywords::find().count(&self.db).await.unwrap_or(0);
            let user_count = Users::find().count(&self.db).await.unwrap_or(0);

            let groups: u64 = {
                let sql = "SELECT COUNT(DISTINCT group_id) as count FROM messages WHERE group_id IS NOT NULL";
                self.db
                    .query_one(Statement::from_string(DbBackend::Sqlite, sql))
                    .await
                    .ok()
                    .flatten()
                    .and_then(|r| r.try_get::<i64>("", "count").ok())
                    .unwrap_or(0) as u64
            };

            StorageStats {
                total_messages: msg_count,
                total_keywords: kw_count,
                total_users: user_count,
                groups_tracked: groups,
            }
        }

        /// 搜索包含特定关键词的消息
        pub async fn search_messages(
            &self,
            group_id: i64,
            keyword: &str,
            limit: u64,
        ) -> anyhow::Result<Vec<messages::Model>> {
            let results = Messages::find()
                .filter(messages::Column::GroupId.eq(group_id))
                .filter(messages::Column::CleanText.contains(keyword))
                .order_by_desc(messages::Column::CreatedAt)
                .limit(limit)
                .all(&self.db)
                .await?;
            Ok(results)
        }

        /// 获取某用户的消息历史
        pub async fn user_messages(
            &self,
            user_id: i64,
            group_id: Option<i64>,
            limit: u64,
        ) -> anyhow::Result<Vec<messages::Model>> {
            let mut query = Messages::find().filter(messages::Column::UserId.eq(user_id));

            if let Some(gid) = group_id {
                query = query.filter(messages::Column::GroupId.eq(gid));
            }

            let results = query
                .order_by_desc(messages::Column::CreatedAt)
                .limit(limit)
                .all(&self.db)
                .await?;
            Ok(results)
        }
    }
}

// =============================
//      Main Plugin Logic
// =============================

use kovi::PluginBuilder;
use std::sync::Arc;
use tokio::sync::OnceCell;

static LOGGER: OnceCell<Arc<db::Logger>> = OnceCell::const_new();

/// 获取 Logger 实例，供外部插件调用
pub async fn get_logger() -> Option<Arc<db::Logger>> {
    LOGGER.get().cloned()
}

#[kovi::plugin]
async fn main() {
    let bot = PluginBuilder::get_runtime_bot();
    let data_dir = bot.get_data_path();

    // 初始化配置
    let config_lock = config::Config::load(data_dir.clone());
    config::CONFIG.set(config_lock.clone()).ok();

    // 初始化数据库
    let logger = Arc::new(db::Logger::new(data_dir).await);
    LOGGER.set(logger.clone()).ok();

    kovi::log::info!("[msg-logger] 消息记录器已启动");

    PluginBuilder::on_msg(move |event| {
        let logger = logger.clone();
        let config_lock = config_lock.clone();

        async move {
            // 判断是否需要记录（快速读取配置，立即释放锁）
            let should_record = {
                let cfg = config_lock.read();
                match event.group_id {
                    Some(gid) => cfg.should_record_group(gid),
                    None => cfg.should_record_private(),
                }
            };

            if should_record {
                let log_event = event.clone();
                let log_logger = logger.clone();
                kovi::tokio::spawn(async move {
                    if let Err(e) = log_logger.log_message(&log_event).await {
                        kovi::log::error!("[msg-logger] 记录失败: {}", e);
                    }
                });
            }

            // 处理管理命令
            let text = match event.borrow_text() {
                Some(t) => t.trim(),
                None => return,
            };

            if !event.is_group() {
                return;
            }

            let group_id = event.group_id.unwrap();

            match text {
                "开启记录" => {
                    let msg = {
                        let mut cfg = config_lock.write();
                        cfg.enable_group(group_id)
                    };
                    event.reply(msg);
                }
                "关闭记录" => {
                    let msg = {
                        let mut cfg = config_lock.write();
                        cfg.disable_group(group_id)
                    };
                    event.reply(msg);
                }
                "记录状态" => {
                    handle_status(group_id, &event, &logger, &config_lock).await;
                }
                "本群词云" => {
                    handle_word_cloud(group_id, &event, &logger).await;
                }
                "本群热力图" => {
                    handle_heatmap(group_id, &event, &logger).await;
                }
                "龙王榜" => {
                    handle_top_talkers(group_id, &event, &logger).await;
                }
                _ => {}
            }
        }
    });
}

async fn handle_status(
    group_id: i64,
    event: &Arc<kovi::MsgEvent>,
    logger: &Arc<db::Logger>,
    config_lock: &Arc<parking_lot::RwLock<config::Config>>,
) {
    let stats = logger.query().storage_stats().await;

    // 快速读取配置状态
    let status = {
        let cfg = config_lock.read();
        if cfg.should_record_group(group_id) {
            "🟢 开启中"
        } else {
            "🔴 关闭中"
        }
    };

    let msg = format!(
        "📊 记录状态: {}\n\
         📚 总消息: {}\n\
         🔠 总词汇: {}\n\
         👥 总用户: {}\n\
         💬 追踪群数: {}",
        status, stats.total_messages, stats.total_keywords, stats.total_users, stats.groups_tracked
    );
    event.reply(msg);
}

async fn handle_word_cloud(group_id: i64, event: &Arc<kovi::MsgEvent>, logger: &Arc<db::Logger>) {
    match logger.query().word_cloud(group_id, 20, 7).await {
        Ok(words) if words.is_empty() => {
            event.reply("📭 数据不足，无法生成词云");
        }
        Ok(words) => {
            let mut out = String::from("☁️ 本群热词 Top 20 (近7天)\n");
            for (i, w) in words.iter().enumerate() {
                out.push_str(&format!("{}. {} ({})\n", i + 1, w.word, w.count));
            }
            event.reply(out);
        }
        Err(e) => {
            event.reply(format!("❌ 查询失败: {}", e));
        }
    }
}

async fn handle_heatmap(group_id: i64, event: &Arc<kovi::MsgEvent>, logger: &Arc<db::Logger>) {
    match logger.query().hourly_heatmap(group_id, 30).await {
        Ok(stats) if stats.is_empty() => {
            event.reply("📭 数据不足");
        }
        Ok(stats) => {
            let max_count = stats.iter().map(|s| s.count).max().unwrap_or(1) as f64;
            let mut out = String::from("🕐 24小时活跃分布 (近30天)\n");
            for s in &stats {
                let bar_len = ((s.count as f64 / max_count) * 10.0) as usize;
                let bar: String = "█".repeat(bar_len);
                out.push_str(&format!("{:02}时 {} {}\n", s.hour, bar, s.count));
            }
            event.reply(out);
        }
        Err(e) => {
            event.reply(format!("❌ 查询失败: {}", e));
        }
    }
}

async fn handle_top_talkers(group_id: i64, event: &Arc<kovi::MsgEvent>, logger: &Arc<db::Logger>) {
    match logger.query().top_talkers(group_id, 10, 7).await {
        Ok(users) if users.is_empty() => {
            event.reply("📭 数据不足");
        }
        Ok(users) => {
            let mut out = String::from("🏆 本群龙王榜 Top 10 (近7天)\n");
            for (i, u) in users.iter().enumerate() {
                let medal = match i {
                    0 => "🥇",
                    1 => "🥈",
                    2 => "🥉",
                    _ => "  ",
                };
                out.push_str(&format!(
                    "{} {}. {} - {} 条\n",
                    medal,
                    i + 1,
                    u.nickname,
                    u.message_count
                ));
            }
            event.reply(out);
        }
        Err(e) => {
            event.reply(format!("❌ 查询失败: {}", e));
        }
    }
}
