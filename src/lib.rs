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
//! if let Some(logger) = kovi_plugin_msg_logger::get_logger().await {
//!     // 词云数据
//!     let words = logger.query().word_cloud(group_id, 20, 7).await?;
//!     // 二维热力图 (星期×小时)
//!     let heatmap = logger.query().weekly_hourly_heatmap(group_id, 30).await?;
//!     // 用户个人统计
//!     let stats = logger.query().user_stats(user_id, Some(group_id)).await?;
//!     // 消息类型分布
//!     let types = logger.query().message_type_stats(group_id, 7).await?;
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

# 管理员列表 (可以使用开启/关闭记录命令)
admins = []

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
        #[serde(default)]
        pub admins: Vec<i64>,
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

        /// 检查用户是否是管理员（配置文件中的管理员 OR 全局Bot管理员 OR 群管理员/群主）
        pub fn is_admin(
            &self,
            user_id: i64,
            sender_role: Option<&str>,
            bot_admins: &[i64],
        ) -> bool {
            // 1. 检查插件配置文件的 admins
            if self.admins.contains(&user_id) {
                return true;
            }
            // 2. 检查 Kovi Bot 本体的全局管理员
            if bot_admins.contains(&user_id) {
                return true;
            }
            // 3. 检查群内权限
            matches!(sender_role, Some("admin") | Some("owner"))
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
    use kovi::MsgEvent;
    use kovi::chrono::{Datelike, NaiveDate, TimeZone, Timelike};
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

            let _ = db
                .execute(builder.build(schema.create_table_from_entity(Messages).if_not_exists()))
                .await;
            let _ = db
                .execute(builder.build(schema.create_table_from_entity(Keywords).if_not_exists()))
                .await;
            let _ = db
                .execute(builder.build(schema.create_table_from_entity(Users).if_not_exists()))
                .await;

            let indexes = [
                "CREATE INDEX IF NOT EXISTS idx_messages_group_id ON messages(group_id)",
                "CREATE INDEX IF NOT EXISTS idx_messages_user_id ON messages(user_id)",
                "CREATE INDEX IF NOT EXISTS idx_messages_created_at ON messages(created_at)",
                "CREATE INDEX IF NOT EXISTS idx_messages_group_time ON messages(group_id, created_at)",
                "CREATE INDEX IF NOT EXISTS idx_messages_group_user_time ON messages(group_id, user_id, created_at)",
                "CREATE INDEX IF NOT EXISTS idx_messages_dow_hour ON messages(day_of_week, hour_of_day)",
                "CREATE INDEX IF NOT EXISTS idx_keywords_word ON keywords(word)",
                "CREATE INDEX IF NOT EXISTS idx_keywords_group_id ON keywords(group_id)",
                "CREATE INDEX IF NOT EXISTS idx_keywords_group_time ON keywords(group_id, created_at)",
                "CREATE INDEX IF NOT EXISTS idx_keywords_user_id ON keywords(user_id)",
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

        pub fn query(&self) -> &QueryApi {
            &self.query_api
        }

        pub async fn log_message(&self, event: &Arc<MsgEvent>) -> anyhow::Result<()> {
            let created_at = event.time;
            let datetime = kovi::chrono::Local
                .timestamp_opt(created_at, 0)
                .single()
                .unwrap_or_else(kovi::chrono::Local::now);
            let hour_of_day = datetime.hour() as i32;
            let day_of_week = datetime.weekday().num_days_from_sunday() as i32;

            let msg_text = event.borrow_text().unwrap_or("").to_string();
            let raw_json = event.original_json.to_string();

            let has_image = raw_json.contains("\"type\":\"image\"");
            let has_at = raw_json.contains("\"type\":\"at\"");
            let is_reply = raw_json.contains("\"type\":\"reply\"");

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

            self.upsert_user(event, created_at).await?;

            let tokenizer_snapshot = {
                let cfg = config::get();
                let cfg_read = cfg.read();
                TokenizerSnapshot::from_config(&cfg_read)
            };

            if tokenizer_snapshot.enabled && !msg_text.trim().is_empty() {
                let jieba = self.jieba.clone();
                let group_id = event.group_id;
                let user_id = event.user_id;

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
            let existing = Users::find_by_id(event.user_id).one(&self.db).await?;

            match existing {
                Some(user) => {
                    let new_count = user.message_count + 1;
                    let mut active: users::ActiveModel = user.into();
                    active.nickname = ActiveValue::Set(nickname);
                    active.last_seen = ActiveValue::Set(timestamp);
                    active.message_count = ActiveValue::Set(new_count);
                    active.update(&self.db).await?;
                }
                None => {
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

    // =============================
    //       Query API Types
    // =============================

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

    /// 消息类型分布
    #[derive(Debug, Clone, Default)]
    pub struct MessageTypeStats {
        pub text_only: i64,  // 纯文字消息
        pub with_image: i64, // 包含图片
        pub with_at: i64,    // 包含 @
        pub with_reply: i64, // 回复消息
        pub total: i64,
    }

    /// 用户个人统计
    #[derive(Debug, Clone)]
    pub struct UserPersonalStats {
        pub user_id: i64,
        pub nickname: String,
        pub total_messages: i64,
        pub total_words: i64,
        pub avg_msg_length: f64,
        pub first_seen: i64,
        pub last_seen: i64,
        pub active_days: i64,
        pub favorite_hour: Option<i32>,
        pub rank_in_group: Option<i64>,
    }

    /// 时段对比结果
    #[derive(Debug, Clone)]
    pub struct PeriodComparison {
        pub current_count: i64,
        pub previous_count: i64,
        pub change_rate: f64, // 变化百分比
    }

    // =============================
    //       Query API Implementation
    // =============================

    #[derive(Clone)]
    pub struct QueryApi {
        db: DatabaseConnection,
    }

    impl QueryApi {
        /// 计算时间戳范围 (start_date 00:00:00 到 end_date 23:59:59)
        fn date_range_to_timestamps(start: NaiveDate, end: NaiveDate) -> (i64, i64) {
            use kovi::chrono::{Local, NaiveTime};

            let start_dt = start.and_time(NaiveTime::MIN);
            let end_dt = end
                .and_hms_opt(23, 59, 59)
                .unwrap_or(end.and_time(NaiveTime::MIN));

            let tz = Local::now().timezone();
            let start_ts = tz
                .from_local_datetime(&start_dt)
                .single()
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
            let end_ts = tz
                .from_local_datetime(&end_dt)
                .single()
                .map(|dt| dt.timestamp())
                .unwrap_or(i64::MAX);

            (start_ts, end_ts)
        }

        /// 获取词云数据（基于天数，从今天往前）
        pub async fn word_cloud(
            &self,
            group_id: i64,
            limit: u64,
            days: i64,
        ) -> anyhow::Result<Vec<WordCount>> {
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

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

        /// 获取词云数据（基于日期范围）
        pub async fn word_cloud_range(
            &self,
            group_id: i64,
            limit: u64,
            start_date: NaiveDate,
            end_date: NaiveDate,
        ) -> anyhow::Result<Vec<WordCount>> {
            let (start_ts, end_ts) = Self::date_range_to_timestamps(start_date, end_date);

            let sql = format!(
                "SELECT word, COUNT(*) as count FROM keywords \
                 WHERE group_id = {} AND created_at >= {} AND created_at <= {} \
                 GROUP BY word ORDER BY count DESC LIMIT {}",
                group_id, start_ts, end_ts, limit
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

        /// 获取24小时活跃分布
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

        /// 获取二维热力图数据 (星期 × 小时)
        pub async fn weekly_hourly_heatmap(
            &self,
            group_id: i64,
            days: i64,
        ) -> anyhow::Result<[[i64; 24]; 7]> {
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT day_of_week, hour_of_day, COUNT(*) as count FROM messages \
                 WHERE group_id = {} AND created_at >= {} \
                 GROUP BY day_of_week, hour_of_day",
                group_id, start_time
            );

            let rows = self
                .db
                .query_all(Statement::from_string(DbBackend::Sqlite, sql))
                .await?;

            let mut grid = [[0i64; 24]; 7];
            for row in rows {
                let dow: i32 = row.try_get("", "day_of_week")?;
                let hour: i32 = row.try_get("", "hour_of_day")?;
                let count: i64 = row.try_get("", "count")?;
                if (0..7).contains(&dow) && (0..24).contains(&hour) {
                    grid[dow as usize][hour as usize] = count;
                }
            }
            Ok(grid)
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

        /// 获取每日消息趋势（基于天数）
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

        /// 获取每日消息趋势（基于日期范围）
        pub async fn daily_trend_range(
            &self,
            group_id: i64,
            start_date: NaiveDate,
            end_date: NaiveDate,
        ) -> anyhow::Result<Vec<DailyStats>> {
            let (start_ts, end_ts) = Self::date_range_to_timestamps(start_date, end_date);

            let sql = format!(
                "SELECT date(created_at, 'unixepoch', 'localtime') as date, COUNT(*) as count \
                 FROM messages WHERE group_id = {} AND created_at >= {} AND created_at <= {} \
                 GROUP BY date ORDER BY date",
                group_id, start_ts, end_ts
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

        /// 获取活跃用户排行
        pub async fn top_talkers(
            &self,
            group_id: i64,
            limit: u64,
            days: i64,
        ) -> anyhow::Result<Vec<UserActivity>> {
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT m.user_id, COALESCE(u.nickname, m.sender_nickname, '') as nickname, COUNT(*) as count \
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

        /// 获取活跃用户排行（基于日期范围）
        pub async fn top_talkers_range(
            &self,
            group_id: i64,
            limit: u64,
            start_date: NaiveDate,
            end_date: NaiveDate,
        ) -> anyhow::Result<Vec<UserActivity>> {
            let (start_ts, end_ts) = Self::date_range_to_timestamps(start_date, end_date);

            let sql = format!(
                "SELECT m.user_id, COALESCE(u.nickname, m.sender_nickname, '') as nickname, COUNT(*) as count \
                 FROM messages m \
                 LEFT JOIN users u ON m.user_id = u.user_id \
                 WHERE m.group_id = {} AND m.created_at >= {} AND m.created_at <= {} \
                 GROUP BY m.user_id ORDER BY count DESC LIMIT {}",
                group_id, start_ts, end_ts, limit
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

        /// 获取消息类型分布
        pub async fn message_type_stats(
            &self,
            group_id: i64,
            days: i64,
        ) -> anyhow::Result<MessageTypeStats> {
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT \
                    COUNT(*) as total, \
                    SUM(CASE WHEN has_image = 0 AND has_at = 0 AND is_reply = 0 THEN 1 ELSE 0 END) as text_only, \
                    SUM(CASE WHEN has_image = 1 THEN 1 ELSE 0 END) as with_image, \
                    SUM(CASE WHEN has_at = 1 THEN 1 ELSE 0 END) as with_at, \
                    SUM(CASE WHEN is_reply = 1 THEN 1 ELSE 0 END) as with_reply \
                 FROM messages \
                 WHERE group_id = {} AND created_at >= {}",
                group_id, start_time
            );

            let row = self
                .db
                .query_one(Statement::from_string(DbBackend::Sqlite, sql))
                .await?
                .ok_or_else(|| anyhow::anyhow!("No data"))?;

            Ok(MessageTypeStats {
                total: row.try_get("", "total").unwrap_or(0),
                text_only: row.try_get("", "text_only").unwrap_or(0),
                with_image: row.try_get("", "with_image").unwrap_or(0),
                with_at: row.try_get("", "with_at").unwrap_or(0),
                with_reply: row.try_get("", "with_reply").unwrap_or(0),
            })
        }

        /// 获取用户个人统计
        pub async fn user_stats(
            &self,
            user_id: i64,
            group_id: Option<i64>,
        ) -> anyhow::Result<UserPersonalStats> {
            let group_filter = match group_id {
                Some(gid) => format!("AND m.group_id = {}", gid),
                None => String::new(),
            };

            // 基本统计
            let sql = format!(
                "SELECT \
                    COUNT(*) as total_messages, \
                    COALESCE(AVG(text_length), 0) as avg_length, \
                    MIN(m.created_at) as first_seen, \
                    MAX(m.created_at) as last_seen, \
                    COUNT(DISTINCT date(m.created_at, 'unixepoch', 'localtime')) as active_days \
                 FROM messages m \
                 WHERE m.user_id = {} {}",
                user_id, group_filter
            );

            let row = self
                .db
                .query_one(Statement::from_string(DbBackend::Sqlite, sql.clone()))
                .await?
                .ok_or_else(|| anyhow::anyhow!("User not found"))?;

            let total_messages: i64 = row.try_get("", "total_messages").unwrap_or(0);
            let avg_msg_length: f64 = row.try_get("", "avg_length").unwrap_or(0.0);
            let first_seen: i64 = row.try_get("", "first_seen").unwrap_or(0);
            let last_seen: i64 = row.try_get("", "last_seen").unwrap_or(0);
            let active_days: i64 = row.try_get("", "active_days").unwrap_or(0);

            // 获取昵称
            let nickname = Users::find_by_id(user_id)
                .one(&self.db)
                .await?
                .map(|u| u.nickname)
                .unwrap_or_default();

            // 获取词汇总数
            let kw_sql = format!(
                "SELECT COUNT(*) as count FROM keywords WHERE user_id = {} {}",
                user_id,
                group_id
                    .map(|gid| format!("AND group_id = {}", gid))
                    .unwrap_or_default()
            );
            let total_words: i64 = self
                .db
                .query_one(Statement::from_string(DbBackend::Sqlite, kw_sql))
                .await?
                .and_then(|r| r.try_get("", "count").ok())
                .unwrap_or(0);

            // 获取最活跃时段
            let hour_sql = format!(
                "SELECT hour_of_day, COUNT(*) as count FROM messages \
                 WHERE user_id = {} {} \
                 GROUP BY hour_of_day ORDER BY count DESC LIMIT 1",
                user_id, group_filter
            );
            let favorite_hour: Option<i32> = self
                .db
                .query_one(Statement::from_string(DbBackend::Sqlite, hour_sql))
                .await?
                .and_then(|r| r.try_get("", "hour_of_day").ok());

            // 获取群内排名（仅当指定了 group_id）
            let rank_in_group = if let Some(gid) = group_id {
                let rank_sql = format!(
                    "SELECT COUNT(*) + 1 as rank FROM ( \
                        SELECT user_id, COUNT(*) as cnt FROM messages \
                        WHERE group_id = {} GROUP BY user_id \
                    ) WHERE cnt > ( \
                        SELECT COUNT(*) FROM messages WHERE group_id = {} AND user_id = {} \
                    )",
                    gid, gid, user_id
                );
                self.db
                    .query_one(Statement::from_string(DbBackend::Sqlite, rank_sql))
                    .await?
                    .and_then(|r| r.try_get("", "rank").ok())
            } else {
                None
            };

            Ok(UserPersonalStats {
                user_id,
                nickname,
                total_messages,
                total_words,
                avg_msg_length,
                first_seen,
                last_seen,
                active_days,
                favorite_hour,
                rank_in_group,
            })
        }

        /// 获取时段对比数据
        pub async fn period_comparison(
            &self,
            group_id: i64,
            current_start: NaiveDate,
            current_end: NaiveDate,
            previous_start: NaiveDate,
            previous_end: NaiveDate,
        ) -> anyhow::Result<PeriodComparison> {
            let (cur_start_ts, cur_end_ts) =
                Self::date_range_to_timestamps(current_start, current_end);
            let (prev_start_ts, prev_end_ts) =
                Self::date_range_to_timestamps(previous_start, previous_end);

            let sql = format!(
                "SELECT \
                    (SELECT COUNT(*) FROM messages WHERE group_id = {} AND created_at >= {} AND created_at <= {}) as current_count, \
                    (SELECT COUNT(*) FROM messages WHERE group_id = {} AND created_at >= {} AND created_at <= {}) as previous_count",
                group_id, cur_start_ts, cur_end_ts, group_id, prev_start_ts, prev_end_ts
            );

            let row = self
                .db
                .query_one(Statement::from_string(DbBackend::Sqlite, sql))
                .await?
                .ok_or_else(|| anyhow::anyhow!("Query failed"))?;

            let current_count: i64 = row.try_get("", "current_count").unwrap_or(0);
            let previous_count: i64 = row.try_get("", "previous_count").unwrap_or(0);

            let change_rate = if previous_count > 0 {
                ((current_count - previous_count) as f64 / previous_count as f64) * 100.0
            } else if current_count > 0 {
                100.0
            } else {
                0.0
            };

            Ok(PeriodComparison {
                current_count,
                previous_count,
                change_rate,
            })
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
    // 克隆 bot 实例以便传入闭包
    let bot_clone = bot.clone();

    let data_dir = bot.get_data_path();

    let config_lock = config::Config::load(data_dir.clone());
    config::CONFIG.set(config_lock.clone()).ok();

    let logger = Arc::new(db::Logger::new(data_dir).await);
    LOGGER.set(logger.clone()).ok();

    kovi::log::info!("[msg-logger] 消息记录器已启动");

    PluginBuilder::on_msg(move |event| {
        let logger = logger.clone();
        let config_lock = config_lock.clone();
        let bot = bot_clone.clone();

        async move {
            // 判断是否需要记录
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
            let sender_role = event.sender.role.as_deref();

            match text {
                "开启记录" => {
                    let bot_admins = bot.get_all_admin().unwrap_or_default();

                    let is_admin = {
                        let cfg = config_lock.read();
                        // 传入 bot_admins
                        cfg.is_admin(event.user_id, sender_role, &bot_admins)
                    };
                    if !is_admin {
                        event.reply("⚠️ 仅管理员可操作");
                        return;
                    }
                    let msg = {
                        let mut cfg = config_lock.write();
                        cfg.enable_group(group_id)
                    };
                    event.reply(msg);
                }
                "关闭记录" => {
                    // 获取全局管理员列表
                    let bot_admins = bot.get_all_admin().unwrap_or_default();

                    let is_admin = {
                        let cfg = config_lock.read();
                        // 传入 bot_admins
                        cfg.is_admin(event.user_id, sender_role, &bot_admins)
                    };
                    if !is_admin {
                        event.reply("⚠️ 仅管理员可操作");
                        return;
                    }
                    let msg = {
                        let mut cfg = config_lock.write();
                        cfg.disable_group(group_id)
                    };
                    event.reply(msg);
                }
                "记录状态" => {
                    handle_status(group_id, &event, &logger, &config_lock).await;
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
