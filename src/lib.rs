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

    /// 配置快照，用于避免长时间持有锁
    #[derive(Clone)]
    pub struct ConfigSnapshot {
        pub mode: RecordMode,
        pub record_private: bool,
        pub admins: Vec<i64>,
        pub whitelist: Vec<i64>,
        pub blacklist: Vec<i64>,
        pub tokenizer_enabled: bool,
        pub min_word_length: usize,
        pub stop_words: HashSet<String>,
    }

    impl ConfigSnapshot {
        pub fn from_config(cfg: &Config) -> Self {
            Self {
                mode: cfg.mode.clone(),
                record_private: cfg.record_private,
                admins: cfg.admins.clone(),
                whitelist: cfg.groups.whitelist.clone(),
                blacklist: cfg.groups.blacklist.clone(),
                tokenizer_enabled: cfg.tokenizer.enabled,
                min_word_length: cfg.tokenizer.min_word_length,
                stop_words: cfg.stop_words_set.clone(),
            }
        }

        pub fn should_record_group(&self, group_id: i64) -> bool {
            match self.mode {
                RecordMode::Whitelist => self.whitelist.contains(&group_id),
                RecordMode::Blacklist => !self.blacklist.contains(&group_id),
            }
        }

        pub fn should_record_private(&self) -> bool {
            self.record_private
        }

        pub fn is_admin(
            &self,
            user_id: i64,
            sender_role: Option<&str>,
            bot_admins: &[i64],
        ) -> bool {
            if self.admins.contains(&user_id) {
                return true;
            }
            if bot_admins.contains(&user_id) {
                return true;
            }
            matches!(sender_role, Some("admin") | Some("owner"))
        }

        pub fn is_stop_word(&self, word: &str) -> bool {
            self.stop_words.contains(word)
        }
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

        /// 快速获取快照，最小化锁持有时间
        pub fn snapshot(&self) -> ConfigSnapshot {
            ConfigSnapshot::from_config(self)
        }

        /// 检查用户是否是管理员（配置文件中的管理员 OR 全局Bot管理员 OR 群管理员/群主）
        pub fn is_admin(
            &self,
            user_id: i64,
            sender_role: Option<&str>,
            bot_admins: &[i64],
        ) -> bool {
            if self.admins.contains(&user_id) {
                return true;
            }
            if bot_admins.contains(&user_id) {
                return true;
            }
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
    use super::config::{self};
    use super::entities::{prelude::*, *};
    use jieba_rs::Jieba;
    use kovi::MsgEvent;
    use kovi::chrono::{Datelike, NaiveDate, TimeZone, Timelike};
    use parking_lot::Mutex;
    use sea_orm::prelude::Expr;
    use sea_orm::sea_query::OnConflict;
    use sea_orm::{
        ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, Database, DatabaseConnection,
        DbBackend, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Schema,
        Statement, TransactionTrait,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;
    use tokio::sync::mpsc;

    // =============================
    //       查询限制常量
    // =============================

    /// 查询限制常量
    pub mod limits {
        /// 词云最大返回数量
        pub const MAX_WORD_CLOUD_LIMIT: u64 = 200;
        /// 用户排行最大返回数量
        pub const MAX_TOP_TALKERS_LIMIT: u64 = 100;
        /// 搜索消息最大返回数量
        pub const MAX_SEARCH_LIMIT: u64 = 500;
        /// 用户消息历史最大返回数量
        pub const MAX_USER_MESSAGES_LIMIT: u64 = 1000;
        /// 最大查询天数
        pub const MAX_QUERY_DAYS: i64 = 365;
        /// 排名计算最大扫描用户数
        pub const MAX_RANK_SCAN_USERS: i64 = 10000;
        /// 默认查询超时（秒）
        pub const DEFAULT_QUERY_TIMEOUT_SECS: u64 = 30;
        /// 批量写入缓冲区大小
        pub const WRITE_BUFFER_SIZE: usize = 1000;
        /// 批量写入阈值
        pub const WRITE_BATCH_THRESHOLD: usize = 50;
        /// 批量写入间隔（毫秒）
        pub const WRITE_FLUSH_INTERVAL_MS: u64 = 500;
    }

    // =============================
    //       查询缓存
    // =============================

    /// 简单的查询缓存
    struct QueryCache<T> {
        data: Option<(T, Instant)>,
        ttl_secs: u64,
    }

    impl<T: Clone> QueryCache<T> {
        fn new(ttl_secs: u64) -> Self {
            Self {
                data: None,
                ttl_secs,
            }
        }

        fn get(&self) -> Option<T> {
            self.data.as_ref().and_then(|(data, time)| {
                if time.elapsed().as_secs() < self.ttl_secs {
                    Some(data.clone())
                } else {
                    None
                }
            })
        }

        fn set(&mut self, value: T) {
            self.data = Some((value, Instant::now()));
        }
    }

    // =============================
    //       批量写入
    // =============================

    /// 待写入的数据
    struct PendingWrite {
        message: messages::ActiveModel,
        keywords: Vec<keywords::ActiveModel>,
        user_upsert: users::ActiveModel,
    }

    /// 消息写入缓冲区
    struct WriteBuffer {
        tx: mpsc::Sender<PendingWrite>,
        #[allow(dead_code)]
        flush_flag: Arc<AtomicBool>,
    }

    impl WriteBuffer {
        fn start(db: DatabaseConnection) -> Self {
            let (tx, mut rx) = mpsc::channel::<PendingWrite>(limits::WRITE_BUFFER_SIZE);
            let flush_flag = Arc::new(AtomicBool::new(false));
            let flush_flag_clone = flush_flag.clone();

            tokio::spawn(async move {
                let mut buffer: Vec<PendingWrite> =
                    Vec::with_capacity(limits::WRITE_BATCH_THRESHOLD * 2);
                let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(
                    limits::WRITE_FLUSH_INTERVAL_MS,
                ));

                loop {
                    tokio::select! {
                        recv_result = rx.recv() => {
                            match recv_result {
                                Some(write) => {
                                    buffer.push(write);
                                    // 达到批量阈值立即写入
                                    if buffer.len() >= limits::WRITE_BATCH_THRESHOLD {
                                        Self::flush_buffer(&db, &mut buffer).await;
                                    }
                                }
                                None => {
                                    // 通道关闭，刷新剩余数据并退出
                                    if !buffer.is_empty() {
                                        Self::flush_buffer(&db, &mut buffer).await;
                                    }
                                    break;
                                }
                            }
                        }
                        _ = interval.tick() => {
                            // 定时刷新
                            if !buffer.is_empty() {
                                Self::flush_buffer(&db, &mut buffer).await;
                            }
                        }
                    }

                    // 检查强制刷新标志
                    if flush_flag_clone.load(Ordering::Relaxed) && !buffer.is_empty() {
                        Self::flush_buffer(&db, &mut buffer).await;
                        flush_flag_clone.store(false, Ordering::Relaxed);
                    }
                }
            });

            WriteBuffer { tx, flush_flag }
        }

        async fn flush_buffer(db: &DatabaseConnection, buffer: &mut Vec<PendingWrite>) {
            if buffer.is_empty() {
                return;
            }

            // 使用事务批量写入
            let txn = match db.begin().await {
                Ok(t) => t,
                Err(e) => {
                    kovi::log::error!("[msg-logger] 事务开始失败: {}", e);
                    // 不清空 buffer，下次重试
                    return;
                }
            };

            let mut success = true;

            for write in buffer.iter() {
                // 插入用户
                if let Err(e) = users::Entity::insert(write.user_upsert.clone())
                    .on_conflict(
                        OnConflict::column(users::Column::UserId)
                            .update_column(users::Column::Nickname)
                            .update_column(users::Column::LastSeen)
                            .value(
                                users::Column::MessageCount,
                                Expr::col(users::Column::MessageCount).add(1),
                            )
                            .to_owned(),
                    )
                    .exec(&txn)
                    .await
                {
                    kovi::log::error!("[msg-logger] 用户写入失败: {}", e);
                    success = false;
                    break;
                }

                // 插入消息
                if let Err(e) = messages::Entity::insert(write.message.clone())
                    .exec(&txn)
                    .await
                {
                    kovi::log::error!("[msg-logger] 消息写入失败: {}", e);
                    success = false;
                    break;
                }

                // 插入关键词
                if !write.keywords.is_empty()
                    && let Err(e) = keywords::Entity::insert_many(write.keywords.clone())
                        .exec(&txn)
                        .await
                {
                    kovi::log::error!("[msg-logger] 关键词写入失败: {}", e);
                    success = false;
                    break;
                }
            }

            if success {
                if let Err(e) = txn.commit().await {
                    kovi::log::error!("[msg-logger] 事务提交失败: {}", e);
                } else {
                    buffer.clear();
                }
            } else {
                // 回滚事务
                if let Err(e) = txn.rollback().await {
                    kovi::log::error!("[msg-logger] 事务回滚失败: {}", e);
                }
                // 保留 buffer 以便重试，但为防止无限重试，只保留部分
                if buffer.len() > limits::WRITE_BATCH_THRESHOLD {
                    buffer.drain(0..limits::WRITE_BATCH_THRESHOLD);
                }
            }
        }

        async fn send(
            &self,
            write: PendingWrite,
        ) -> Result<(), mpsc::error::SendError<PendingWrite>> {
            self.tx.send(write).await
        }
    }

    // =============================
    //       消息记录器
    // =============================

    /// 消息记录器核心结构
    pub struct Logger {
        db: DatabaseConnection,
        jieba: Arc<Jieba>,
        query_api: QueryApi,
        write_buffer: WriteBuffer,
    }

    impl Logger {
        pub async fn new(data_dir: PathBuf) -> Self {
            if !data_dir.exists() {
                std::fs::create_dir_all(&data_dir).unwrap();
            }
            let db_path = data_dir.join("msg_history.sqlite");
            let db_url = format!("sqlite://{}?mode=rwc", db_path.to_string_lossy());

            let mut opt = sea_orm::ConnectOptions::new(db_url);
            opt.sqlx_logging(false)
                // 连接池配置
                .max_connections(10)
                .min_connections(2)
                .connect_timeout(std::time::Duration::from_secs(10))
                .acquire_timeout(std::time::Duration::from_secs(10))
                .idle_timeout(std::time::Duration::from_secs(300))
                .max_lifetime(std::time::Duration::from_secs(3600));

            let db = Database::connect(opt)
                .await
                .expect("Failed to connect to SQLite");

            Self::init_database(&db).await;

            let jieba = tokio::task::spawn_blocking(Jieba::new)
                .await
                .expect("Failed to initialize Jieba");

            let query_api = QueryApi::new(db.clone());
            let write_buffer = WriteBuffer::start(db.clone());

            Self {
                db,
                jieba: Arc::new(jieba),
                query_api,
                write_buffer,
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
                // 基础索引
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
                "CREATE INDEX IF NOT EXISTS idx_messages_group_user_count ON messages(group_id, user_id)",
                "CREATE INDEX IF NOT EXISTS idx_messages_user_group ON messages(user_id, group_id)",
                "CREATE INDEX IF NOT EXISTS idx_keywords_user_group_time ON keywords(user_id, group_id, created_at)",
                // 用户小时分布索引
                "CREATE INDEX IF NOT EXISTS idx_messages_user_hour ON messages(user_id, hour_of_day)",
            ];

            for sql in indexes {
                let _ = db
                    .execute(Statement::from_string(DbBackend::Sqlite, sql))
                    .await;
            }

            let pragmas = [
                "PRAGMA journal_mode=WAL",
                "PRAGMA synchronous=NORMAL",
                "PRAGMA cache_size=-64000", // 64MB 缓存
                "PRAGMA temp_store=MEMORY",
                "PRAGMA mmap_size=268435456", // 256MB 内存映射
                "PRAGMA busy_timeout=5000",   // 5秒锁等待超时
            ];

            for pragma in pragmas {
                let _ = db
                    .execute(Statement::from_string(DbBackend::Sqlite, pragma))
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

            let mut msg_text = event.borrow_text().unwrap_or("").to_string();
            let mut raw_json = event.original_json.to_string();

            const MAX_TEXT_LEN: usize = 4000;
            const MAX_JSON_LEN: usize = 10000;

            if msg_text.len() > MAX_TEXT_LEN {
                msg_text.truncate(MAX_TEXT_LEN);
                msg_text.push_str("...(truncated)");
            }

            if raw_json.len() > MAX_JSON_LEN {
                raw_json.truncate(MAX_JSON_LEN);
                raw_json.push_str("...(truncated)");
            }

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

            // 准备用户数据
            let nickname = event.sender.nickname.clone().unwrap_or_default();
            let user_model = users::ActiveModel {
                user_id: ActiveValue::Set(event.user_id),
                nickname: ActiveValue::Set(nickname),
                first_seen: ActiveValue::Set(created_at),
                last_seen: ActiveValue::Set(created_at),
                message_count: ActiveValue::Set(1),
            };

            // 获取配置快照（快速释放锁）
            let snapshot = {
                let cfg = config::get();
                let cfg_read = cfg.read();
                cfg_read.snapshot()
            };

            // 准备关键词数据
            let keywords = if snapshot.tokenizer_enabled && !msg_text.trim().is_empty() {
                let jieba = self.jieba.clone();
                let group_id = event.group_id;
                let user_id = event.user_id;
                let min_len = snapshot.min_word_length;

                let keywords_data = tokio::task::spawn_blocking(move || {
                    let words = jieba.cut(&msg_text, true);
                    let max_word_len = 20;

                    // 使用 HashMap 聚合相同词，去重
                    let mut word_set: HashMap<String, i32> = HashMap::new();

                    for w in words {
                        let s = w.trim();
                        let len = s.chars().count();
                        if len >= min_len && len <= max_word_len && !snapshot.is_stop_word(s) {
                            word_set.entry(s.to_string()).or_insert(len as i32);
                        }
                    }

                    word_set.into_iter().collect::<Vec<_>>()
                })
                .await?;

                keywords_data
                    .into_iter()
                    .map(|(word, word_length)| keywords::ActiveModel {
                        message_id: ActiveValue::Set(0), // 将在批量写入时更新
                        word: ActiveValue::Set(word),
                        word_length: ActiveValue::Set(word_length),
                        group_id: ActiveValue::Set(group_id),
                        user_id: ActiveValue::Set(user_id),
                        created_at: ActiveValue::Set(created_at),
                        ..Default::default()
                    })
                    .collect()
            } else {
                Vec::new()
            };

            // 发送到写入缓冲区
            let pending = PendingWrite {
                message: msg_model,
                keywords,
                user_upsert: user_model,
            };

            if let Err(e) = self.write_buffer.send(pending).await {
                // 如果缓冲区满，回退到直接写入
                kovi::log::warn!("[msg-logger] 写入缓冲区满，直接写入: {}", e);
                self.direct_write(event, created_at, hour_of_day, day_of_week)
                    .await?;
            }

            Ok(())
        }

        /// 直接写入（回退方案）
        async fn direct_write(
            &self,
            event: &Arc<MsgEvent>,
            created_at: i64,
            hour_of_day: i32,
            day_of_week: i32,
        ) -> anyhow::Result<()> {
            let mut msg_text = event.borrow_text().unwrap_or("").to_string();
            let mut raw_json = event.original_json.to_string();

            const MAX_TEXT_LEN: usize = 4000;
            const MAX_JSON_LEN: usize = 10000;

            if msg_text.len() > MAX_TEXT_LEN {
                msg_text.truncate(MAX_TEXT_LEN);
                msg_text.push_str("...(truncated)");
            }

            if raw_json.len() > MAX_JSON_LEN {
                raw_json.truncate(MAX_JSON_LEN);
                raw_json.push_str("...(truncated)");
            }

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

            // Upsert 用户
            let nickname = event.sender.nickname.clone().unwrap_or_default();
            let user_model = users::ActiveModel {
                user_id: ActiveValue::Set(event.user_id),
                nickname: ActiveValue::Set(nickname),
                first_seen: ActiveValue::Set(created_at),
                last_seen: ActiveValue::Set(created_at),
                message_count: ActiveValue::Set(1),
            };

            users::Entity::insert(user_model)
                .on_conflict(
                    OnConflict::column(users::Column::UserId)
                        .update_column(users::Column::Nickname)
                        .update_column(users::Column::LastSeen)
                        .value(
                            users::Column::MessageCount,
                            Expr::col(users::Column::MessageCount).add(1),
                        )
                        .to_owned(),
                )
                .exec(&self.db)
                .await?;

            // 插入消息
            let inserted = msg_model.insert(&self.db).await?;
            let db_id = inserted.id;

            // 获取配置快照
            let snapshot = {
                let cfg = config::get();
                let cfg_read = cfg.read();
                cfg_read.snapshot()
            };

            if snapshot.tokenizer_enabled && !msg_text.trim().is_empty() {
                let jieba = self.jieba.clone();
                let group_id = event.group_id;
                let user_id = event.user_id;
                let min_len = snapshot.min_word_length;

                let keywords_data = tokio::task::spawn_blocking(move || {
                    let words = jieba.cut(&msg_text, true);
                    let max_word_len = 20;

                    let mut word_set: HashMap<String, i32> = HashMap::new();

                    for w in words {
                        let s = w.trim();
                        let len = s.chars().count();
                        if len >= min_len && len <= max_word_len && !snapshot.is_stop_word(s) {
                            word_set.entry(s.to_string()).or_insert(len as i32);
                        }
                    }

                    word_set.into_iter().collect::<Vec<_>>()
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
        pub text_only: i64,
        pub with_image: i64,
        pub with_at: i64,
        pub with_reply: i64,
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
        pub change_rate: f64,
    }

    // =============================
    //       Query API Implementation
    // =============================

    #[derive(Clone)]
    pub struct QueryApi {
        db: DatabaseConnection,
        storage_stats_cache: Arc<Mutex<QueryCache<StorageStats>>>,
    }

    impl QueryApi {
        fn new(db: DatabaseConnection) -> Self {
            Self {
                db,
                storage_stats_cache: Arc::new(Mutex::new(QueryCache::new(60))), // 60秒缓存
            }
        }

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

        /// 带超时的查询执行
        async fn query_with_timeout<T, F, Fut>(&self, f: F) -> anyhow::Result<T>
        where
            F: FnOnce() -> Fut,
            Fut: std::future::Future<Output = anyhow::Result<T>>,
        {
            let timeout = tokio::time::Duration::from_secs(limits::DEFAULT_QUERY_TIMEOUT_SECS);
            tokio::time::timeout(timeout, f()).await.map_err(|_| {
                anyhow::anyhow!(
                    "Query timeout after {}s",
                    limits::DEFAULT_QUERY_TIMEOUT_SECS
                )
            })?
        }

        /// 获取词云数据（基于天数，从今天往前）
        pub async fn word_cloud(
            &self,
            group_id: i64,
            limit: u64,
            days: i64,
        ) -> anyhow::Result<Vec<WordCount>> {
            let limit = limit.min(limits::MAX_WORD_CLOUD_LIMIT);
            let days = days.min(limits::MAX_QUERY_DAYS);
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT word, COUNT(*) as count FROM keywords \
                 WHERE group_id = {} AND created_at >= {} \
                 GROUP BY word ORDER BY count DESC LIMIT {}",
                group_id, start_time, limit
            );

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
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
            })
            .await
        }

        /// 获取词云数据（基于日期范围）
        pub async fn word_cloud_range(
            &self,
            group_id: i64,
            limit: u64,
            start_date: NaiveDate,
            end_date: NaiveDate,
        ) -> anyhow::Result<Vec<WordCount>> {
            let limit = limit.min(limits::MAX_WORD_CLOUD_LIMIT);
            let (start_ts, end_ts) = Self::date_range_to_timestamps(start_date, end_date);

            let sql = format!(
                "SELECT word, COUNT(*) as count FROM keywords \
                 WHERE group_id = {} AND created_at >= {} AND created_at <= {} \
                 GROUP BY word ORDER BY count DESC LIMIT {}",
                group_id, start_ts, end_ts, limit
            );

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
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
            })
            .await
        }

        /// 获取用户专属词云
        pub async fn user_word_cloud(
            &self,
            user_id: i64,
            group_id: Option<i64>,
            limit: u64,
            days: i64,
        ) -> anyhow::Result<Vec<WordCount>> {
            let limit = limit.min(limits::MAX_WORD_CLOUD_LIMIT);
            let days = days.min(limits::MAX_QUERY_DAYS);
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

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
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
            })
            .await
        }

        /// 获取24小时活跃分布
        pub async fn hourly_heatmap(
            &self,
            group_id: i64,
            days: i64,
        ) -> anyhow::Result<Vec<HourlyStats>> {
            let days = days.min(limits::MAX_QUERY_DAYS);
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT hour_of_day as hour, COUNT(*) as count FROM messages \
                 WHERE group_id = {} AND created_at >= {} \
                 GROUP BY hour_of_day ORDER BY hour_of_day",
                group_id, start_time
            );

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
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
            })
            .await
        }

        /// 获取二维热力图数据 (星期 × 小时)
        pub async fn weekly_hourly_heatmap(
            &self,
            group_id: i64,
            days: i64,
        ) -> anyhow::Result<[[i64; 24]; 7]> {
            let days = days.min(limits::MAX_QUERY_DAYS);
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT day_of_week, hour_of_day, COUNT(*) as count FROM messages \
                 WHERE group_id = {} AND created_at >= {} \
                 GROUP BY day_of_week, hour_of_day",
                group_id, start_time
            );

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
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
            })
            .await
        }

        /// 获取星期活跃分布
        pub async fn weekly_distribution(
            &self,
            group_id: i64,
            days: i64,
        ) -> anyhow::Result<Vec<(i32, i64)>> {
            let days = days.min(limits::MAX_QUERY_DAYS);
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT day_of_week, COUNT(*) as count FROM messages \
                 WHERE group_id = {} AND created_at >= {} \
                 GROUP BY day_of_week ORDER BY day_of_week",
                group_id, start_time
            );

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
                    .query_all(Statement::from_string(DbBackend::Sqlite, sql))
                    .await?;

                let mut result = Vec::with_capacity(rows.len());
                for row in rows {
                    let dow: i32 = row.try_get("", "day_of_week")?;
                    let count: i64 = row.try_get("", "count")?;
                    result.push((dow, count));
                }
                Ok(result)
            })
            .await
        }

        /// 获取每日消息趋势（基于天数）
        pub async fn daily_trend(
            &self,
            group_id: i64,
            days: i64,
        ) -> anyhow::Result<Vec<DailyStats>> {
            let days = days.min(limits::MAX_QUERY_DAYS);
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT date(created_at, 'unixepoch', 'localtime') as date, COUNT(*) as count \
                 FROM messages WHERE group_id = {} AND created_at >= {} \
                 GROUP BY date ORDER BY date",
                group_id, start_time
            );

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
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
            })
            .await
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

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
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
            })
            .await
        }

        /// 获取活跃用户排行
        pub async fn top_talkers(
            &self,
            group_id: i64,
            limit: u64,
            days: i64,
        ) -> anyhow::Result<Vec<UserActivity>> {
            let limit = limit.min(limits::MAX_TOP_TALKERS_LIMIT);
            let days = days.min(limits::MAX_QUERY_DAYS);
            let start_time = kovi::chrono::Local::now().timestamp() - (days * 86400);

            let sql = format!(
                "SELECT m.user_id, COALESCE(u.nickname, m.sender_nickname, '') as nickname, COUNT(*) as count \
                 FROM messages m \
                 LEFT JOIN users u ON m.user_id = u.user_id \
                 WHERE m.group_id = {} AND m.created_at >= {} \
                 GROUP BY m.user_id ORDER BY count DESC LIMIT {}",
                group_id, start_time, limit
            );

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
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
            })
            .await
        }

        /// 获取活跃用户排行（基于日期范围）
        pub async fn top_talkers_range(
            &self,
            group_id: i64,
            limit: u64,
            start_date: NaiveDate,
            end_date: NaiveDate,
        ) -> anyhow::Result<Vec<UserActivity>> {
            let limit = limit.min(limits::MAX_TOP_TALKERS_LIMIT);
            let (start_ts, end_ts) = Self::date_range_to_timestamps(start_date, end_date);

            let sql = format!(
                "SELECT m.user_id, COALESCE(u.nickname, m.sender_nickname, '') as nickname, COUNT(*) as count \
                 FROM messages m \
                 LEFT JOIN users u ON m.user_id = u.user_id \
                 WHERE m.group_id = {} AND m.created_at >= {} AND m.created_at <= {} \
                 GROUP BY m.user_id ORDER BY count DESC LIMIT {}",
                group_id, start_ts, end_ts, limit
            );

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
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
            })
            .await
        }

        /// 获取消息类型分布
        pub async fn message_type_stats(
            &self,
            group_id: i64,
            days: i64,
        ) -> anyhow::Result<MessageTypeStats> {
            let days = days.min(limits::MAX_QUERY_DAYS);
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

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let row = db
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
            })
            .await
        }

        /// 获取用户个人统计（带超时保护）
        pub async fn user_stats(
            &self,
            user_id: i64,
            group_id: Option<i64>,
        ) -> anyhow::Result<UserPersonalStats> {
            self.query_with_timeout(|| self.user_stats_inner(user_id, group_id))
                .await
        }

        /// 用户统计内部实现
        async fn user_stats_inner(
            &self,
            user_id: i64,
            group_id: Option<i64>,
        ) -> anyhow::Result<UserPersonalStats> {
            let group_filter = match group_id {
                Some(gid) => format!("AND m.group_id = {}", gid),
                None => String::new(),
            };

            let kw_group_filter = match group_id {
                Some(gid) => format!("AND group_id = {}", gid),
                None => String::new(),
            };

            // 合并多个查询为一个，减少数据库往返
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
                .query_one(Statement::from_string(DbBackend::Sqlite, sql))
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
                user_id, kw_group_filter
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

            // 排名计算
            let rank_in_group = if let Some(gid) = group_id {
                self.calculate_user_rank(gid, user_id, total_messages).await
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

        /// 用户排名计算
        async fn calculate_user_rank(
            &self,
            group_id: i64,
            user_id: i64,
            user_msg_count: i64,
        ) -> Option<i64> {
            let _ = user_id;
            if user_msg_count == 0 {
                return None;
            }

            // 只统计比该用户消息多的用户数量，添加 LIMIT 防止全表扫描
            let rank_sql = format!(
                "SELECT COUNT(*) as rank FROM ( \
                    SELECT user_id FROM messages \
                    WHERE group_id = {} \
                    GROUP BY user_id \
                    HAVING COUNT(*) > {} \
                    LIMIT {} \
                )",
                group_id,
                user_msg_count,
                limits::MAX_RANK_SCAN_USERS
            );

            let rank: i64 = self
                .db
                .query_one(Statement::from_string(DbBackend::Sqlite, rank_sql))
                .await
                .ok()?
                .and_then(|r| r.try_get("", "rank").ok())
                .unwrap_or(0);

            Some(rank + 1)
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

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let row = db
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
            })
            .await
        }

        /// 获取用户在各群的活跃度
        pub async fn user_group_activity(&self, user_id: i64) -> anyhow::Result<Vec<(i64, i64)>> {
            let sql = format!(
                "SELECT group_id, COUNT(*) as count FROM messages \
                 WHERE user_id = {} AND group_id IS NOT NULL \
                 GROUP BY group_id ORDER BY count DESC LIMIT {}",
                user_id,
                limits::MAX_TOP_TALKERS_LIMIT
            );

            let db = self.db.clone();
            self.query_with_timeout(|| async {
                let rows = db
                    .query_all(Statement::from_string(DbBackend::Sqlite, sql))
                    .await?;

                let mut result = Vec::with_capacity(rows.len());
                for row in rows {
                    let gid: i64 = row.try_get("", "group_id")?;
                    let count: i64 = row.try_get("", "count")?;
                    result.push((gid, count));
                }
                Ok(result)
            })
            .await
        }

        /// 获取存储统计概况（带缓存）
        pub async fn storage_stats(&self) -> StorageStats {
            // 先检查缓存
            {
                let cache = self.storage_stats_cache.lock();
                if let Some(cached) = cache.get() {
                    return cached;
                }
            }

            // 执行实际查询
            let stats = self.storage_stats_uncached().await;

            // 更新缓存
            {
                let mut cache = self.storage_stats_cache.lock();
                cache.set(stats.clone());
            }

            stats
        }

        /// 不带缓存的存储统计查询
        async fn storage_stats_uncached(&self) -> StorageStats {
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
            let limit = limit.min(limits::MAX_SEARCH_LIMIT);

            let db = self.db.clone();
            let keyword = keyword.to_string();

            self.query_with_timeout(|| async {
                let results = Messages::find()
                    .filter(messages::Column::GroupId.eq(group_id))
                    .filter(messages::Column::CleanText.contains(&keyword))
                    .order_by_desc(messages::Column::CreatedAt)
                    .limit(limit)
                    .all(&db)
                    .await?;
                Ok(results)
            })
            .await
        }

        /// 获取某用户的消息历史
        pub async fn user_messages(
            &self,
            user_id: i64,
            group_id: Option<i64>,
            limit: u64,
        ) -> anyhow::Result<Vec<messages::Model>> {
            let limit = limit.min(limits::MAX_USER_MESSAGES_LIMIT);

            let db = self.db.clone();

            self.query_with_timeout(|| async {
                let mut query = Messages::find().filter(messages::Column::UserId.eq(user_id));

                if let Some(gid) = group_id {
                    query = query.filter(messages::Column::GroupId.eq(gid));
                }

                let results = query
                    .order_by_desc(messages::Column::CreatedAt)
                    .limit(limit)
                    .all(&db)
                    .await?;
                Ok(results)
            })
            .await
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
            // 一次性获取快照，立即释放锁
            let snapshot = {
                let cfg = config_lock.read();
                cfg.snapshot()
            }; // 锁在这里立即释放

            // 判断是否需要记录（使用快照，无锁）
            let should_record = match event.group_id {
                Some(gid) => snapshot.should_record_group(gid),
                None => snapshot.should_record_private(),
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
            let bot_admins = bot.get_all_admin().unwrap_or_default();

            match text {
                "开启记录" => {
                    // 使用快照检查权限（无锁）
                    if !snapshot.is_admin(event.user_id, sender_role, &bot_admins) {
                        event.reply("⚠️ 仅管理员可操作");
                        return;
                    }
                    // 只有需要修改时才获取写锁
                    let msg = {
                        let mut cfg = config_lock.write();
                        cfg.enable_group(group_id)
                    };
                    event.reply(msg);
                }
                "关闭记录" => {
                    if !snapshot.is_admin(event.user_id, sender_role, &bot_admins) {
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
                    handle_status(group_id, &event, &logger, &snapshot).await;
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
    snapshot: &config::ConfigSnapshot,
) {
    let stats = logger.query().storage_stats().await;

    let status = if snapshot.should_record_group(group_id) {
        "🟢 开启中"
    } else {
        "🔴 关闭中"
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
