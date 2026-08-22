use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::extract_token_from_header;
use crate::plans::Plan;
use crate::routes::AppState;

#[derive(Deserialize)]
pub struct PipelineRequest {
    #[serde(default)]
    pub requests: Vec<HranaRequest>,
}

#[derive(Deserialize)]
pub struct HranaRequest {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub stmt: Option<HranaStmt>,
    #[serde(default)]
    pub batch: Option<HranaBatch>,
}

#[derive(Deserialize, Clone)]
pub struct HranaStmt {
    pub sql: String,
    #[serde(default)]
    pub args: Option<Value>,
    #[serde(default)]
    pub named_args: Option<Value>,
}

#[derive(Deserialize, Clone)]
pub struct HranaBatch {
    #[serde(default)]
    pub steps: Vec<HranaStep>,
}

#[derive(Deserialize, Clone)]
pub struct HranaStep {
    #[serde(default)]
    pub stmt: Option<HranaStmt>,
}

type ApiError = (StatusCode, Json<Value>);

fn api_err(status: StatusCode, msg: &str) -> ApiError {
    (status, Json(json!({ "error": msg })))
}

fn value_to_turso(v: &Value) -> Result<turso::Value, String> {
    match v {
        Value::Null => Ok(turso::Value::Null),
        Value::Bool(b) => Ok(turso::Value::Integer(*b as i64)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(turso::Value::Integer(i))
            } else {
                n.as_f64().map(turso::Value::Real).ok_or_else(|| "invalid number".to_string())
            }
        }
        Value::String(s) => Ok(turso::Value::Text(s.clone())),
        Value::Array(_) | Value::Object(_) => Err("unsupported argument type".to_string()),
    }
}

fn arg_to_turso(v: &Value) -> Result<turso::Value, String> {
    if let Some(obj) = v.as_object() {
        let t = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
        return match t {
            "null" => Ok(turso::Value::Null),
            "integer" => obj
                .get("value")
                .and_then(|x| x.as_i64())
                .or_else(|| obj.get("value").and_then(|x| x.as_str()).and_then(|s| s.parse().ok()))
                .map(turso::Value::Integer)
                .ok_or_else(|| "invalid integer arg".to_string()),
            "float" => obj
                .get("value")
                .and_then(|x| x.as_f64())
                .map(turso::Value::Real)
                .ok_or_else(|| "invalid float arg".to_string()),
            "text" | "string" => obj
                .get("value")
                .and_then(|x| x.as_str())
                .map(|s| turso::Value::Text(s.to_string()))
                .ok_or_else(|| "invalid text arg".to_string()),
            "blob" => {
                let raw = obj.get("value").and_then(|x| x.as_str()).unwrap_or("");
                let bytes = base64::engine::general_purpose::STANDARD.decode(raw).map_err(|e| e.to_string())?;
                Ok(turso::Value::Blob(bytes))
            }
            _ => value_to_turso(v),
        };
    }
    value_to_turso(v)
}

fn stmt_params(stmt: &HranaStmt) -> Result<turso::params::Params, String> {
    if let Some(named) = &stmt.named_args {
        if let Some(map) = named.as_object() {
            let mut out: Vec<(String, turso::Value)> = Vec::with_capacity(map.len());
            for (k, v) in map {
                out.push((k.clone(), arg_to_turso(v)?));
            }
            return Ok(turso::params::Params::Named(out));
        }
    }
    match &stmt.args {
        None => Ok(turso::params::Params::None),
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for v in arr {
                out.push(arg_to_turso(v)?);
            }
            Ok(turso::params::Params::Positional(out))
        }
        Some(Value::Object(map)) => {
            let mut out: Vec<(String, turso::Value)> = Vec::with_capacity(map.len());
            for (k, v) in map {
                out.push((k.clone(), arg_to_turso(v)?));
            }
            Ok(turso::params::Params::Named(out))
        }
        Some(other) => Err(format!("unsupported args payload: {}", other)),
    }
}

fn value_to_json(v: turso::Value) -> Value {
    match v {
        turso::Value::Null => Value::Null,
        turso::Value::Integer(n) => json!(n),
        turso::Value::Real(f) => json!(f),
        turso::Value::Text(s) => json!(s),
        turso::Value::Blob(b) => json!(format!("blob:{}bytes", b.len())),
    }
}

fn stmt_ok(cols: Vec<(String, Option<String>)>, rows: Vec<Vec<turso::Value>>, affected: u64) -> Value {
    let cols_json: Vec<Value> = cols
        .into_iter()
        .map(|(name, decl)| json!({ "name": name, "decltype": decl }))
        .collect();
    let rows_json: Vec<Value> = rows
        .into_iter()
        .map(|r| Value::Array(r.into_iter().map(value_to_json).collect()))
        .collect();
    json!({
        "type": "ok",
        "response": { "result": {
            "cols": cols_json,
            "rows": rows_json,
            "affected_row_count": affected,
            "last_insert_rowid": null,
        }}
    })
}

fn stmt_err(msg: &str) -> Value {
    json!({ "type": "error", "error": { "message": msg } })
}

pub async fn pipeline_handler(
    state: State<AppState>,
    headers: HeaderMap,
    Path(target): Path<String>,
    Json(body): Json<PipelineRequest>,
) -> Result<Json<Value>, ApiError> {
    let token = extract_token_from_header(&headers).ok_or_else(|| api_err(StatusCode::UNAUTHORIZED, "Missing bearer token"))?;
    let user = state
        .user_store
        .find_by_api_key(&token)
        .await
        .ok_or_else(|| api_err(StatusCode::UNAUTHORIZED, "Invalid API key"))?;

    let admin = crate::auth::is_admin(&user.username);
    let databases = state.db_manager.list_databases(if admin { None } else { Some(&user.username) });
    let db_id = databases
        .iter()
        .find(|(id, entry)| entry.name == target || id.as_str() == target)
        .map(|(id, _)| id.clone())
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Database not found"))?;

    let plan = Plan::from_str(&user.plan);
    if state
        .rate_limiter
        .check_with_limit(&user.username, plan.max_queries_per_minute())
        .is_err()
    {
        return Err(api_err(StatusCode::TOO_MANY_REQUESTS, "Query rate limit exceeded for your plan"));
    }

    let mut results: Vec<Value> = Vec::with_capacity(body.requests.len());

    for req in &body.requests {
        match req.kind.as_str() {
            "execute" => {
                let Some(stmt) = &req.stmt else {
                    results.push(stmt_err("missing stmt for execute"));
                    continue;
                };
                state.query_tracker.track_query(&user.username);
                match stmt_params(stmt) {
                    Ok(p) => match state.db_manager.run_statement(&db_id, &stmt.sql, p).await {
                        Ok((cols, rows, affected)) => results.push(stmt_ok(cols, rows, affected)),
                        Err(e) => results.push(stmt_err(&e.to_string())),
                    },
                    Err(e) => results.push(stmt_err(&e)),
                }
            }
            "batch" => {
                let Some(batch) = &req.batch else {
                    results.push(stmt_err("missing batch payload"));
                    continue;
                };
                let mut step_results = Vec::with_capacity(batch.steps.len());
                let step_errors = vec![Value::Null; batch.steps.len()];
                for step in &batch.steps {
                    let Some(stmt) = &step.stmt else {
                        step_results.push(stmt_err("missing stmt for batch step"));
                        continue;
                    };
                    state.query_tracker.track_query(&user.username);
                    match stmt_params(stmt) {
                        Ok(p) => match state.db_manager.run_statement(&db_id, &stmt.sql, p).await {
                            Ok((cols, rows, affected)) => step_results.push(stmt_ok(cols, rows, affected)),
                            Err(e) => step_results.push(stmt_err(&e.to_string())),
                        },
                        Err(e) => step_results.push(stmt_err(&e)),
                    }
                }
                results.push(json!({
                    "type": "ok",
                    "response": { "result": { "step_results": step_results, "step_errors": step_errors } }
                }));
            }
            "close" => {
                results.push(json!({ "type": "ok" }));
            }
            other => {
                tracing::warn!("Unsupported libsql request type: {}", other);
                results.push(stmt_err(&format!("unsupported request type '{}'", other)));
            }
        }
    }

    Ok(Json(json!({ "results": results, "baton": null })))
}
