use actix_multipart::Multipart;
use actix_web::{middleware, web, App, Error as AWError, HttpResponse, HttpServer};
use bytes::BytesMut;
use futures::TryStreamExt;
use listenfd::ListenFd;
use mysql::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::Mutex;


#[macro_use]
mod newrelic_util;

type Pool = r2d2::Pool<r2d2_mysql::MysqlConnectionManager>;
type BlockingDBError = actix_web::error::BlockingError<mysql::Error>;

const LIMIT: i64 = 20;
const NAZOTTE_LIMIT: usize = 50;

#[derive(Debug)]
struct MySQLConnectionEnv {
    host: String,
    port: u16,
    user: String,
    db_name: String,
    password: String,
}

#[derive(Debug)]
struct MultiMySQLConnectionEnv {
    chair: MySQLConnectionEnv,
    estate: MySQLConnectionEnv,
}

impl MySQLConnectionEnv {
    fn chair() -> Self {
        let port = if let Ok(port) = env::var("CHAIR_MYSQL_PORT") {
            port.parse().unwrap_or(3306)
        } else {
            3306
        };
        Self {
            host: env::var("CHAIR_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned()),
            port,
            user: env::var("CHAIR_MYSQL_USER").unwrap_or_else(|_| "isucon".to_owned()),
            db_name: env::var("CHAIR_MYSQL_DBNAME").unwrap_or_else(|_| "isuumo".to_owned()),
            password: env::var("CHAIR_MYSQL_PASS").unwrap_or_else(|_| "isucon".to_owned()),
        }
    }

    fn estate() -> Self {
        let port = if let Ok(port) = env::var("ESTATE_MYSQL_PORT") {
            port.parse().unwrap_or(3306)
        } else {
            3306
        };
        Self {
            host: env::var("ESTATE_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned()),
            port,
            user: env::var("ESTATE_MYSQL_USER").unwrap_or_else(|_| "isucon".to_owned()),
            db_name: env::var("ESTATE_MYSQL_DBNAME").unwrap_or_else(|_| "isuumo".to_owned()),
            password: env::var("ESTATE_MYSQL_PASS").unwrap_or_else(|_| "isucon".to_owned()),
        }
    }
}

impl Default for MultiMySQLConnectionEnv {
    fn default() -> Self {
        MultiMySQLConnectionEnv{
            chair: MySQLConnectionEnv::chair(),
            estate: MySQLConnectionEnv::estate(),
        }
    }
}

struct AppCache {
    low_priced_estates: Mutex<Vec<Estate>>,
}

#[derive(Clone)]
struct MultiPool {
    chair: Pool,
    estate: Pool,
}

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "actix_server=info,actix_web=info,isuumo=info");
    }
    env_logger::init();

    let mysql_connection_env = Arc::new(MultiMySQLConnectionEnv::default());
    let chair_search_condition: Arc<ChairSearchCondition> = {
        let file = File::open("../fixture/chair_condition.json")?;
        Arc::new(serde_json::from_reader(file)?)
    };
    let estate_search_condition: Arc<EstateSearchCondition> = {
        let file = File::open("../fixture/estate_condition.json")?;
        Arc::new(serde_json::from_reader(file)?)
    };

    let manager_chair = r2d2_mysql::MysqlConnectionManager::new(
        mysql::OptsBuilder::new()
            .ip_or_hostname(Some(&mysql_connection_env.chair.host))
            .tcp_port(mysql_connection_env.chair.port)
            .user(Some(&mysql_connection_env.chair.user))
            .db_name(Some(&mysql_connection_env.chair.db_name))
            .pass(Some(&mysql_connection_env.chair.password)),
    );
    let pool_chair = r2d2::Pool::builder()
        .max_size(10)
        .connection_timeout(std::time::Duration::from_secs(300))
        .build(manager_chair)
        .expect("Failed to create connection pool for chair");

    let manager_estate = r2d2_mysql::MysqlConnectionManager::new(
        mysql::OptsBuilder::new()
            .ip_or_hostname(Some(&mysql_connection_env.estate.host))
            .tcp_port(mysql_connection_env.estate.port)
            .user(Some(&mysql_connection_env.estate.user))
            .db_name(Some(&mysql_connection_env.estate.db_name))
            .pass(Some(&mysql_connection_env.estate.password)),
    );
    let pool_estate = r2d2::Pool::builder()
        .max_size(10)
        .connection_timeout(std::time::Duration::from_secs(300))
        .build(manager_estate)
        .expect("Failed to create connection pool for estate");

    let initial_estates = pool_estate.get()
        .expect("Failed to checkout database connection")
        .exec("select * from estate order by rent asc, id asc limit ?",
              (LIMIT,),)
        .expect("Failed to fetch lower price estates at app start");
    
    let app_cache = web::Data::new(AppCache {
        low_priced_estates: Mutex::new(initial_estates),
    });
    
    let pool = MultiPool{
        chair: pool_chair,
        estate: pool_estate,
    };

    newrelic_init!();

    let mut listenfd = ListenFd::from_env();
    let server = HttpServer::new(move || {
        App::new()
            .data(pool.clone())
            .data(mysql_connection_env.clone())
            .data(chair_search_condition.clone())
            .data(estate_search_condition.clone())
            .app_data(app_cache.clone())
            .wrap(middleware::Logger::default())
            .route("/initialize", web::post().to(initialize))
            .service(
                web::scope("/api")
                    .service(
                        web::scope("/chair")
                            .route("/search", web::get().to(search_chairs))
                            .route("/low_priced", web::get().to(get_low_priced_chair))
                            .route(
                                "/search/condition",
                                web::get().to(get_chair_search_condition),
                            )
                            .route("/buy/{id}", web::post().to(buy_chair))
                            .route("/{id}", web::get().to(get_chair_detail))
                            .route("", web::post().to(post_chair)),
                    )
                    .service(
                        web::scope("/estate")
                            .route("/search", web::get().to(search_estates))
                            .route("/low_priced", web::get().to(get_low_priced_estate))
                            .route(
                                "/req_doc/{id}",
                                web::post().to(post_estate_request_document),
                            )
                            .route("/nazotte", web::post().to(search_estate_nazotte))
                            .route(
                                "/search/condition",
                                web::get().to(get_estate_search_condition),
                            )
                            .route("/{id}", web::get().to(get_estate_detail))
                            .route("", web::post().to(post_estate)),
                    )
                    .route(
                        "/recommended_estate/{id}",
                        web::get().to(search_recommended_estate_with_chair),
                    ),
            )
    });
    let server = if let Some(l) = listenfd.take_tcp_listener(0)? {
        server.listen(l)?
    } else {
        server.bind((
            "0.0.0.0",
            std::env::var("SERVER_PORT")
                .map(|port_str| port_str.parse().expect("Failed to parse SERVER_PORT"))
                .unwrap_or(1323),
        ))?
    };
    server.run().await
}

#[derive(Debug, Deserialize, Serialize)]
struct ChairSearchCondition {
    width: RangeCondition,
    height: RangeCondition,
    depth: RangeCondition,
    price: RangeCondition,
    color: ListCondition,
    feature: ListCondition,
    kind: ListCondition,
}

#[derive(Debug, Deserialize, Serialize)]
struct RangeCondition {
    prefix: String,
    suffix: String,
    ranges: Vec<Range>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Range {
    id: i64,
    min: i64,
    max: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct ListCondition {
    list: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct EstateSearchCondition {
    #[serde(rename = "doorWidth")]
    door_width: RangeCondition,
    #[serde(rename = "doorHeight")]
    door_height: RangeCondition,
    rent: RangeCondition,
    feature: ListCondition,
}

#[derive(Debug, Serialize)]
struct InitializeResponse {
    language: String,
}

async fn initialize(
    db: web::Data<MultiPool>,
    data: web::Data<AppCache>,
    mysql_connection_env: web::Data<Arc<MultiMySQLConnectionEnv>>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("POST /initialize");

    let sql_dir = std::path::Path::new("..").join("mysql").join("db");
    let paths = [
        sql_dir.join("0_Schema.sql"),
        sql_dir.join("1_DummyEstateData.sql"),
        sql_dir.join("2_DummyChairData.sql"),
    ];
    for env in &[&mysql_connection_env.chair, &mysql_connection_env.estate] {
        for p in paths.iter() {
            let sql_file = p.canonicalize().unwrap();
            let cmd_str = format!(
                "mysql -h {} -P {} -u {} -p{} {} < {}",
                env.host,
                env.port,
                env.user,
                env.password,
                env.db_name,
                sql_file.display()
            );
            let status = tokio::process::Command::new("bash")
                .arg("-c")
                .arg(cmd_str)
                .status()
                .await
                .map_err(|e| {
                    log::error!("Initialize script {} failed : {:?}", p.display(), e);
                    HttpResponse::InternalServerError()
                })?;
            if !status.success() {
                log::error!("Initialize script {} failed", p.display());
                return Ok(HttpResponse::InternalServerError().finish());
            }
        }
    }
    {
        // initialize low_priced_estates
        let estates = web::block(move || {
            let mut conn = db.estate.get().expect("Failed to checkout database connection");
            conn.exec(
                "select * from estate order by rent asc, id asc limit ?",
                (LIMIT,),
            )
        })
        .await
        .map_err(|e| {
            log::error!("get_low_priced_estate DB execution error : {:?}", e);
            HttpResponse::InternalServerError()
        })?;
        let mut cache = data.low_priced_estates.lock().unwrap();
        *cache = estates;
    }
    Ok(HttpResponse::Ok().json(InitializeResponse {
        language: "rust".to_owned(),
    }))
}

#[derive(Debug, Serialize, Deserialize)]
struct Chair {
    id: i64,
    name: String,
    description: String,
    thumbnail: String,
    price: i64,
    height: i64,
    width: i64,
    depth: i64,
    color: String,
    features: String,
    kind: String,
    #[serde(skip)]
    popularity: i64,
    #[serde(skip)]
    stock: i64,
}

impl FromRow for Chair {
    fn from_row_opt(row: mysql::Row) -> Result<Self, mysql::FromRowError> {
        fn convert(row: &mysql::Row) -> Result<Chair, ()> {
            Ok(Chair {
                id: row.get("id").ok_or(())?,
                name: row.get("name").ok_or(())?,
                description: row.get("description").ok_or(())?,
                thumbnail: row.get("thumbnail").ok_or(())?,
                price: row.get("price").ok_or(())?,
                height: row.get("height").ok_or(())?,
                width: row.get("width").ok_or(())?,
                depth: row.get("depth").ok_or(())?,
                color: row.get("color").ok_or(())?,
                features: row.get("features").ok_or(())?,
                kind: row.get("kind").ok_or(())?,
                popularity: row.get("popularity").ok_or(())?,
                stock: row.get("stock").ok_or(())?,
            })
        }
        convert(&row).map_err(|_| mysql::FromRowError(row))
    }
}

async fn get_chair_detail(
    db: web::Data<MultiPool>,
    path: web::Path<(i64,)>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("GET /api/chair/{id}");

    let id = path.0;

    let chair: Option<Chair> = web::block(move || {
        let mut conn = db.chair.get().expect("Failed to checkout database connection");
        conn.exec_first("select * from chair where id = ?", (id,))
    })
    .await
    .map_err(|e| {
        log::error!("Failed to get the chair from id : {}", e);
        HttpResponse::InternalServerError()
    })?;

    if let Some(chair) = chair {
        if chair.stock <= 0 {
            log::info!("requested id's chair is sold out : {}", id);
            Ok(HttpResponse::NotFound().finish())
        } else {
            Ok(HttpResponse::Ok().json(chair))
        }
    } else {
        log::info!("requested id's chair not found : {}", id);
        Ok(HttpResponse::NotFound().finish())
    }
}

#[derive(Debug, Deserialize)]
struct CSVChair {
    id: i64,
    name: String,
    description: String,
    thumbnail: String,
    price: i64,
    height: i64,
    width: i64,
    depth: i64,
    color: String,
    features: String,
    kind: String,
    popularity: i64,
    stock: i64,
}

impl Into<Chair> for CSVChair {
    fn into(self) -> Chair {
        Chair {
            id: self.id,
            name: self.name,
            description: self.description,
            thumbnail: self.thumbnail,
            price: self.price,
            height: self.height,
            width: self.width,
            depth: self.depth,
            color: self.color,
            features: self.features,
            kind: self.kind,
            popularity: self.popularity,
            stock: self.stock,
        }
    }
}

async fn post_chair(db: web::Data<MultiPool>, mut payload: Multipart) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("POST /api/chair");

    let mut chairs: Option<Vec<Chair>> = None;
    while let Ok(Some(field)) = payload.try_next().await {
        let content_disposition = field.content_disposition().unwrap();
        let name = content_disposition.get_name().unwrap();
        if name == "chairs" {
            let content = field
                .map_ok(|chunk| BytesMut::from(&chunk[..]))
                .try_concat()
                .await?;
            let mut reader = csv::ReaderBuilder::new()
                .has_headers(false)
                .from_reader(content.as_ref());
            let mut cs = Vec::new();
            for record in reader.deserialize() {
                let chair: CSVChair = record.map_err(|e| {
                    log::error!("failed to read csv: {:?}", e);
                    HttpResponse::InternalServerError()
                })?;
                cs.push(chair.into());
            }
            chairs = Some(cs);
        }
    }
    if chairs.is_none() {
        log::error!("failed to get from file: no chairs given");
        return Ok(HttpResponse::BadRequest().finish());
    }
    let chairs = chairs.unwrap();

    web::block(move || {
        let mut conn = db.chair.get().expect("Failed to checkout database connection");
        let mut tx = conn.start_transaction(mysql::TxOpts::default())?;
        for chair in chairs {
            let params: Vec<mysql::Value> = vec![
                chair.id.into(),
                chair.name.into(),
                chair.description.into(),
                chair.thumbnail.into(),
                chair.price.into(),
                chair.height.into(),
                chair.width.into(),
                chair.depth.into(),
                chair.color.into(),
                chair.features.into(),
                chair.kind.into(),
                chair.popularity.into(),
                chair.stock.into(),
            ];
            tx.exec_drop("insert into chair (id, name, description, thumbnail, price, height, width, depth, color, features, kind, popularity, stock) values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)", params)?;
        }
        tx.commit()?;
        Ok(())
    })
    .await.map_err(|e: BlockingDBError| {
        log::error!("failed to insert/commit chair: {:?}", e);
        HttpResponse::InternalServerError()
    })?;
    Ok(HttpResponse::Created().finish())
}

#[derive(Debug, Deserialize)]
struct SearchChairsParams {
    #[serde(rename = "priceRangeId", default)]
    price_range_id: String,
    #[serde(rename = "heightRangeId", default)]
    height_range_id: String,
    #[serde(rename = "widthRangeId", default)]
    width_range_id: String,
    #[serde(rename = "depthRangeId", default)]
    depth_range_id: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    color: String,
    #[serde(default)]
    features: String,
    page: i64,
    #[serde(rename = "perPage")]
    per_page: i64,
}

#[derive(Debug, Serialize)]
struct ChairSearchResponse {
    count: i64,
    chairs: Vec<Chair>,
}

async fn search_chairs(
    chair_search_condition: web::Data<Arc<ChairSearchCondition>>,
    db: web::Data<MultiPool>,
    query_params: web::Query<SearchChairsParams>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("GET /api/chair/search");

    let mut conditions = Vec::new();
    let mut params: Vec<mysql::Value> = Vec::new();

    if !query_params.price_range_id.is_empty() {
        if let Some(chair_price) =
            get_range(&chair_search_condition.price, &query_params.price_range_id)
        {
            if chair_price.min != -1 {
                conditions.push("price >= ?");
                params.push(chair_price.min.into());
            }
            if chair_price.max != -1 {
                conditions.push("price < ?");
                params.push(chair_price.max.into());
            }
        } else {
            log::info!(
                "priceRangeID invalid, {} : Unexpected Range ID",
                query_params.price_range_id
            );
            return Ok(HttpResponse::BadRequest().finish());
        }
    }

    if !query_params.height_range_id.is_empty() {
        if let Some(chair_height) = get_range(
            &chair_search_condition.height,
            &query_params.height_range_id,
        ) {
            if chair_height.min != -1 {
                conditions.push("height >= ?");
                params.push(chair_height.min.into());
            }
            if chair_height.max != -1 {
                conditions.push("height < ?");
                params.push(chair_height.max.into());
            }
        } else {
            log::info!(
                "heightRangeId invalid, {} : Unexpected Range ID",
                query_params.height_range_id
            );
            return Ok(HttpResponse::BadRequest().finish());
        }
    }

    if !query_params.width_range_id.is_empty() {
        if let Some(chair_width) =
            get_range(&chair_search_condition.width, &query_params.width_range_id)
        {
            if chair_width.min != -1 {
                conditions.push("width >= ?");
                params.push(chair_width.min.into());
            }
            if chair_width.max != -1 {
                conditions.push("width < ?");
                params.push(chair_width.max.into());
            }
        } else {
            log::info!(
                "widthRangeId invalid, {} : Unexpected Range ID",
                query_params.width_range_id
            );
            return Ok(HttpResponse::BadRequest().finish());
        }
    }

    if !query_params.depth_range_id.is_empty() {
        if let Some(chair_depth) =
            get_range(&chair_search_condition.depth, &query_params.depth_range_id)
        {
            if chair_depth.min != -1 {
                conditions.push("depth >= ?");
                params.push(chair_depth.min.into());
            }
            if chair_depth.max != -1 {
                conditions.push("depth < ?");
                params.push(chair_depth.max.into());
            }
        } else {
            log::info!(
                "depthRangeId invalid, {} : Unexpected Range ID",
                query_params.depth_range_id
            );
            return Ok(HttpResponse::BadRequest().finish());
        }
    }

    if !query_params.kind.is_empty() {
        conditions.push("kind = ?");
        params.push(query_params.kind.clone().into());
    }

    if !query_params.color.is_empty() {
        conditions.push("color = ?");
        params.push(query_params.color.clone().into());
    }

    if !query_params.features.is_empty() {
        for f in query_params.features.split(',') {
            conditions.push("features like concat('%', ?, '%')");
            params.push(f.into());
        }
    }

    if conditions.is_empty() {
        log::info!("Search condition not found");
        return Ok(HttpResponse::BadRequest().finish());
    }

    conditions.push("stock > 0");

    let per_page = query_params.per_page;
    let page = query_params.page;

    let search_condition = conditions.join(" and ");
    let res = web::block(move || {
        let mut conn = db.chair.get().expect("Failed to checkout database connection");
        let row = conn.exec_first(
            format!("select count(*) from chair where {}", search_condition),
            &params,
        )?;
        let count = row.map(|(c,)| c).unwrap_or(0);

        params.push(per_page.into());
        params.push((page * per_page).into());
        let chairs = conn.exec(
            format!(
                "select * from chair where {} order by popularity desc, id desc limit ? offset ?",
                search_condition
            ),
            &params,
        )?;
        Ok(ChairSearchResponse { count, chairs })
    })
    .await
    .map_err(|e: BlockingDBError| {
        log::error!("searchChairs DB execution error : {:?}", e);
        HttpResponse::InternalServerError()
    })?;
    Ok(HttpResponse::Ok().json(res))
}

fn get_range<'a>(cond: &'a RangeCondition, range_id: &str) -> Option<&'a Range> {
    range_id.parse().ok().and_then(|range_index| {
        if range_index < 0 || cond.ranges.len() as i64 <= range_index {
            None
        } else {
            Some(&cond.ranges[range_index as usize])
        }
    })
}

#[derive(Debug, Serialize)]
struct ChairListResponse {
    chairs: Vec<Chair>,
}

async fn get_low_priced_chair(db: web::Data<MultiPool>) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("GET /api/chair/low_priced");

    let chairs = web::block(move || {
        let mut conn = db.chair.get().expect("Failed to checkout database connection");
        conn.exec(
            "select * from chair where stock > 0 order by price asc, id asc limit ?",
            (LIMIT,),
        )
    })
    .await
    .map_err(|e| {
        log::error!("get_low_priced_chair DB execution error : {:?}", e);
        HttpResponse::InternalServerError()
    })?;

    Ok(HttpResponse::Ok().json(ChairListResponse { chairs }))
}

async fn get_chair_search_condition(
    chair_search_condition: web::Data<Arc<ChairSearchCondition>>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("GET /api/chair/search/condition");

    Ok(HttpResponse::Ok().json(chair_search_condition.as_ref().as_ref()))
}

#[derive(Debug, Deserialize)]
struct BuyChairRequest {
    email: String,
}

async fn buy_chair(
    db: web::Data<MultiPool>,
    path: web::Path<(i64,)>,
    _params: web::Json<BuyChairRequest>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("POST /api/chair/buy/{id}");

    let id = path.0;

    let found: bool = web::block(move || {
        let mut conn = db.chair.get().expect("Failed to checkout database connection");
        let mut tx = conn.start_transaction(mysql::TxOpts::default())?;
        let row: Option<Chair> = tx.exec_first(
            "select * from chair where id = ? and stock > 0 for update",
            (id,),
        )?;
        if row.is_some() {
            tx.exec_drop("update chair set stock = stock - 1 where id = ?", (id,))?;
            tx.commit()?;
            Ok(true)
        } else {
            Ok(false)
        }
    })
    .await
    .map_err(|e: BlockingDBError| {
        log::error!("buy_chair DB execution error : {:?}", e);
        HttpResponse::InternalServerError()
    })?;

    if found {
        Ok(HttpResponse::Ok().finish())
    } else {
        Ok(HttpResponse::NotFound().finish())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Estate {
    id: i64,
    name: String,
    description: String,
    thumbnail: String,
    address: String,
    latitude: f64,
    longitude: f64,
    rent: i64,
    #[serde(rename = "doorHeight")]
    door_height: i64,
    #[serde(rename = "doorWidth")]
    door_width: i64,
    features: String,
    #[serde(skip)]
    popularity: i64,
}

impl FromRow for Estate {
    fn from_row_opt(row: mysql::Row) -> Result<Self, mysql::FromRowError> {
        fn convert(row: &mysql::Row) -> Result<Estate, ()> {
            Ok(Estate {
                id: row.get("id").ok_or(())?,
                thumbnail: row.get("thumbnail").ok_or(())?,
                name: row.get("name").ok_or(())?,
                description: row.get("description").ok_or(())?,
                latitude: row.get("latitude").ok_or(())?,
                longitude: row.get("longitude").ok_or(())?,
                address: row.get("address").ok_or(())?,
                rent: row.get("rent").ok_or(())?,
                door_height: row.get("door_height").ok_or(())?,
                door_width: row.get("door_width").ok_or(())?,
                features: row.get("features").ok_or(())?,
                popularity: row.get("popularity").ok_or(())?,
            })
        }
        convert(&row).map_err(|_| mysql::FromRowError(row))
    }
}

async fn get_estate_detail(
    db: web::Data<MultiPool>,
    path: web::Path<(i64,)>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("GET /api/estate/{id}");

    let id = path.0;

    let estate: Option<Estate> = web::block(move || {
        let mut conn = db.estate.get().expect("Failed to checkout database connection");
        conn.exec_first("select * from estate where id = ?", (id,))
    })
    .await
    .map_err(|e| {
        log::error!("Database Execution error : {:?}", e);
        HttpResponse::InternalServerError()
    })?;

    if let Some(estate) = estate {
        Ok(HttpResponse::Ok().json(estate))
    } else {
        Ok(HttpResponse::NotFound().finish())
    }
}

#[derive(Debug, Deserialize)]
struct CSVEstate {
    id: i64,
    name: String,
    description: String,
    thumbnail: String,
    address: String,
    latitude: f64,
    longitude: f64,
    rent: i64,
    door_height: i64,
    door_width: i64,
    features: String,
    popularity: i64,
}

impl Into<Estate> for CSVEstate {
    fn into(self) -> Estate {
        Estate {
            id: self.id,
            name: self.name,
            description: self.description,
            thumbnail: self.thumbnail,
            address: self.address,
            latitude: self.latitude,
            longitude: self.longitude,
            rent: self.rent,
            door_height: self.door_height,
            door_width: self.door_width,
            features: self.features,
            popularity: self.popularity,
        }
    }
}

async fn post_estate(
    db: web::Data<MultiPool>, 
    data: web::Data<AppCache>, 
    mut payload: Multipart
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("POST /api/estate");

    let mut estates: Option<Vec<Estate>> = None;
    while let Ok(Some(field)) = payload.try_next().await {
        let content_disposition = field.content_disposition().unwrap();
        let name = content_disposition.get_name().unwrap();
        if name == "estates" {
            let content = field
                .map_ok(|chunk| BytesMut::from(&chunk[..]))
                .try_concat()
                .await?;
            let mut reader = csv::ReaderBuilder::new()
                .has_headers(false)
                .from_reader(content.as_ref());
            let mut es = Vec::new();
            for record in reader.deserialize() {
                let estate: CSVEstate = record.map_err(|e| {
                    log::error!("failed to read csv: {:?}", e);
                    HttpResponse::InternalServerError()
                })?;
                es.push(estate.into());
            }
            estates = Some(es);
        }
    }
    if estates.is_none() {
        log::error!("failed to get from file: no estates given");
        return Ok(HttpResponse::BadRequest().finish());
    }
    let estates = estates.unwrap();

    web::block(move || {
        let mut conn = db.estate.get().expect("Failed to checkout database connection");
        let mut tx = conn.start_transaction(mysql::TxOpts::default())?;
        for estate in estates {
            let query = format!("insert into estate (id, name, description, thumbnail, address, latitude, longitude, rent, door_height, door_width, features, popularity, location) values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, Point({}, {}))", estate.latitude, estate.longitude);
            tx.exec_drop(query, (estate.id, estate.name, estate.description, estate.thumbnail, estate.address, estate.latitude, estate.longitude, estate.rent, estate.door_height, estate.door_width, estate.features, estate.popularity))?;
        }
        tx.commit()?;

        let mut conn = db.estate.get().expect("Failed to checkout database connection");
        let estates = conn.exec(
            "select * from estate order by rent asc, id asc limit ?",
            (LIMIT,),
        )?;
        let mut cache = data.low_priced_estates.lock().unwrap();
        *cache = estates;

        Ok(())
    }).await.map_err(
        |e: BlockingDBError| {
            log::error!("failed to insert/commit estate: {:?}", e);
            HttpResponse::InternalServerError()
        },
    )?;
    Ok(HttpResponse::Created().finish())
}

#[derive(Debug, Deserialize)]
struct SearchEstatesParams {
    #[serde(rename = "doorHeightRangeId", default)]
    door_height_range_id: String,
    #[serde(rename = "doorWidthRangeId", default)]
    door_width_range_id: String,
    #[serde(rename = "rentRangeId", default)]
    rent_range_id: String,
    #[serde(default)]
    features: String,
    page: i64,
    #[serde(rename = "perPage")]
    per_page: i64,
}

#[derive(Debug, Serialize)]
struct EstateSearchResponse {
    count: i64,
    estates: Vec<Estate>,
}

async fn search_estates(
    estate_search_condition: web::Data<Arc<EstateSearchCondition>>,
    db: web::Data<MultiPool>,
    query_params: web::Query<SearchEstatesParams>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("GET /api/estate/search");

    let mut conditions = Vec::new();
    let mut params: Vec<mysql::Value> = Vec::new();

    if !query_params.door_height_range_id.is_empty() {
        if let Some(door_height) = get_range(
            &estate_search_condition.door_height,
            &query_params.door_height_range_id,
        ) {
            if door_height.min != -1 {
                conditions.push("door_height >= ?");
                params.push(door_height.min.into());
            }
            if door_height.max != -1 {
                conditions.push("door_height < ?");
                params.push(door_height.max.into());
            }
        } else {
            log::info!(
                "doorHeightRangeID invalid, {} : Unexpected Range ID",
                query_params.door_height_range_id
            );
            return Ok(HttpResponse::BadRequest().finish());
        }
    }

    if !query_params.door_width_range_id.is_empty() {
        if let Some(door_width) = get_range(
            &estate_search_condition.door_width,
            &query_params.door_width_range_id,
        ) {
            if door_width.min != -1 {
                conditions.push("door_width >= ?");
                params.push(door_width.min.into());
            }
            if door_width.max != -1 {
                conditions.push("door_width < ?");
                params.push(door_width.max.into());
            }
        } else {
            log::info!(
                "doorWidthRangeID invalid, {} : Unexpected Range ID",
                query_params.door_width_range_id
            );
            return Ok(HttpResponse::BadRequest().finish());
        }
    }

    if !query_params.rent_range_id.is_empty() {
        if let Some(estate_rent) =
            get_range(&estate_search_condition.rent, &query_params.rent_range_id)
        {
            if estate_rent.min != -1 {
                conditions.push("rent >= ?");
                params.push(estate_rent.min.into());
            }
            if estate_rent.max != -1 {
                conditions.push("rent < ?");
                params.push(estate_rent.max.into());
            }
        } else {
            log::info!(
                "rentRangeID invalid, {} : Unexpected Range ID",
                query_params.rent_range_id
            );
            return Ok(HttpResponse::BadRequest().finish());
        }
    }

    if !query_params.features.is_empty() {
        for f in query_params.features.split(',') {
            conditions.push("features like concat('%', ?, '%')");
            params.push(f.into());
        }
    }

    if conditions.is_empty() {
        log::info!("search_estates search condition not found");
        return Ok(HttpResponse::BadRequest().finish());
    }

    let per_page = query_params.per_page;
    let page = query_params.page;

    let search_condition = conditions.join(" and ");
    let res = web::block(move || {
        let mut conn = db.estate.get().expect("Failed to checkout database connection");
        let row = conn.exec_first(
            format!("select count(*) from estate where {}", search_condition),
            &params,
        )?;
        let count = row.map(|(c,)| c).unwrap_or(0);

        params.push(per_page.into());
        params.push((page * per_page).into());
        let estates = conn.exec(
            format!(
                "select * from estate where {} order by popularity desc, id desc limit ? offset ?",
                search_condition
            ),
            &params,
        )?;
        Ok(EstateSearchResponse { count, estates })
    })
    .await
    .map_err(|e: BlockingDBError| {
        log::error!("search_estates DB execution error : {:?}", e);
        HttpResponse::InternalServerError()
    })?;
    Ok(HttpResponse::Ok().json(res))
}

#[derive(Debug, Serialize)]
struct EstateListResponse {
    estates: Vec<Estate>,
}

async fn get_low_priced_estate(
    data: web::Data<AppCache>,
    db: web::Data<MultiPool>
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("GET /api/estate/low_priced");

    let cached_estates = &(*data.low_priced_estates.lock().unwrap());
    Ok(HttpResponse::Ok().json(EstateListResponse { estates: cached_estates.to_vec() }))
}

async fn get_estate_search_condition(
    estate_search_condition: web::Data<Arc<EstateSearchCondition>>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("GET /api/estate/search/condition");

    Ok(HttpResponse::Ok().json(estate_search_condition.as_ref().as_ref()))
}

async fn search_recommended_estate_with_chair(
    db: web::Data<MultiPool>,
    path: web::Path<(i64,)>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("GET /api/recommended_estate/{id}");

    let id = path.0;

    let estates = web::block(move || {
        let mut conn_estate = db.estate.get().expect("Failed to checkout database connection");
        let mut conn_chair = db.chair.get().expect("Failed to checkout database connection");
        let chair: Option<Chair> = conn_chair.exec_first("select * from chair where id = ?", (id,))?;
        if let Some(chair) = chair {
            let mut whd = vec![chair.width, chair.height, chair.depth];
            whd.sort();
            let query = "select * from estate where (door_width >= ? and door_height >= ?) or (door_width >= ? and door_height >= ?) order by popularity desc, id desc limit ?";
            let params: Vec<mysql::Value> = vec![
                whd[0].into(),
                whd[1].into(),
                whd[1].into(),
                whd[0].into(),
                LIMIT.into(),
            ];
            Ok(Some(conn_estate.exec(query, params)?))
        } else {
            Ok(None)
        }
    })
    .await
    .map_err(|e: BlockingDBError| {
        log::error!("Database execution error : {:?}", e);
        HttpResponse::InternalServerError()
    })?;

    if let Some(estates) = estates {
        Ok(HttpResponse::Ok().json(EstateListResponse { estates }))
    } else {
        log::info!("Requested chair id \"{}\" not found", id);
        Ok(HttpResponse::BadRequest().finish())
    }
}

#[derive(Debug, Deserialize)]
struct Coordinates {
    coordinates: Vec<Coordinate>,
}

#[derive(Debug, Deserialize)]
struct Coordinate {
    latitude: f64,
    longitude: f64,
}

impl Coordinates {
    fn get_bounding_box(&self) -> BoundingBox {
        let (min_latitude, max_latitude) = self
            .coordinates
            .iter()
            .map(|c| c.latitude)
            .fold((f64::NAN, f64::NAN), |(min, max), val| {
                (min.min(val), max.max(val))
            });
        let (min_longitude, max_longitude) = self
            .coordinates
            .iter()
            .map(|c| c.longitude)
            .fold((f64::NAN, f64::NAN), |(min, max), val| {
                (min.min(val), max.max(val))
            });
        BoundingBox {
            top_left_corner: Coordinate {
                latitude: min_latitude,
                longitude: min_longitude,
            },
            bottom_right_corner: Coordinate {
                latitude: max_latitude,
                longitude: max_longitude,
            },
        }
    }

    fn coordinates_to_text(&self) -> String {
        let points: Vec<_> = self
            .coordinates
            .iter()
            .map(|c| format!("{} {}", c.latitude, c.longitude))
            .collect();
        format!("'POLYGON(({}))'", points.join(","))
    }
}

#[derive(Debug)]
struct BoundingBox {
    top_left_corner: Coordinate,
    bottom_right_corner: Coordinate,
}

async fn search_estate_nazotte(
    db: web::Data<MultiPool>,
    coordinates: web::Json<Coordinates>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("POST /api/estate/nazotte");

    if coordinates.coordinates.is_empty() {
        return Ok(HttpResponse::BadRequest().finish());
    }
    // let bounding_box = coordinates.get_bounding_box();

    let mut estates = web::block(move || {
        let mut conn = db.estate.get().expect("Failed to checkout database connection");
        let query = format!("select * from estate where ST_Contains(ST_PolygonFromText({}), location) order by popularity desc, id desc", coordinates.coordinates_to_text());
        let estates_in_polygon: Vec<Estate> = conn.exec(query, ())?;

        Ok(estates_in_polygon)
    })
    .await
    .map_err(|e: BlockingDBError| {
        log::error!("Database execution error : {:?}", e);
        HttpResponse::InternalServerError()
    })?;

    estates.truncate(NAZOTTE_LIMIT);
    Ok(HttpResponse::Ok().json(EstateSearchResponse {
        count: estates.len() as i64,
        estates,
    }))
}

#[derive(Debug, Deserialize)]
struct PostEstateRequestDocumentParams {
    email: String,
}

async fn post_estate_request_document(
    db: web::Data<MultiPool>,
    path: web::Path<(i64,)>,
    _params: web::Json<PostEstateRequestDocumentParams>,
) -> Result<HttpResponse, AWError> {
    newrelic_transaction!("POST /api/estate/req_doc/{id}");

    let id = path.0;

    let estate: Option<Estate> = web::block(move || {
        let mut conn = db.estate.get().expect("Failed to checkout database connection");
        conn.exec_first("select * from estate where id = ?", (id,))
    })
    .await
    .map_err(|e| {
        log::error!("post_estate_request_document: DB execution error : {:?}", e);
        HttpResponse::InternalServerError()
    })?;

    if estate.is_some() {
        Ok(HttpResponse::Ok().finish())
    } else {
        Ok(HttpResponse::NotFound().finish())
    }
}
