#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use bytes::Bytes;
use prost::Message;
use roaring::RoaringBitmap;
use tracing::{debug, info, warn};

use turso_core::{Connection, Value as CoreValue};
use turso_sync_engine::server_proto::{
    BatchCond, BatchResult, BatchStep, BatchStreamReq, BatchStreamResp, Col, Error,
    ExecuteStreamReq, ExecuteStreamResp, PageData, PageSetRawEncodingProto, PageUpdatesEncodingReq,
    PipelineReqBody, PipelineRespBody, PullUpdatesReqProtoBody, PullUpdatesRespProtoBody, Row,
    StmtResult, StreamRequest, StreamResponse, StreamResult, Value,
};

const WAL_FRAME_HEADER_SIZE: usize = 24;
const PAGE_SIZE: usize = 4096;

struct SyncHub {
    listen: String,
    conn: Arc<Mutex<Arc<Connection>>>,
    token: Option<String>,
}

impl SyncHub {
    fn new(listen: String, conn: Arc<Connection>, token: Option<String>) -> Self {
        conn.wal_auto_actions_disable();
        Self {
            listen,
            conn: Arc::new(Mutex::new(conn)),
            token,
        }
    }

    fn run(&self) -> Result<()> {
        info!("Sync hub listening on {}", self.listen);

        let listener = TcpListener::bind(&self.listen)?;
        listener.set_nonblocking(true)?;

        loop {
            match listener.accept() {
                Ok((stream, addr)) => {
                    debug!("Accepted connection from {addr}");
                    // Accepted sockets inherit the listener's non-blocking
                    // flag on Windows; clear it so blocking reads work.
                    if let Err(e) = stream.set_nonblocking(false) {
                        warn!("Failed to set stream blocking: {e}");
                    }
                    let conn = self.conn.clone();
                    let token = self.token.clone();
                    thread::spawn(move || {
                        if let Err(e) = handle_connection(conn, token, stream) {
                            warn!("Connection from {addr} failed: {e}");
                        }
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(anyhow!("accept failed: {e}")),
            }
        }
    }
}

fn handle_connection(
    conn: Arc<Mutex<Arc<Connection>>>,
    token: Option<String>,
    mut stream: TcpStream,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;

    let mut buffer = [0u8; 8192];
    let mut request_data = Vec::new();

    loop {
        let n = stream.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        request_data.extend_from_slice(&buffer[..n]);

        if let Some(header_end) = find_header_end(&request_data) {
            let headers = String::from_utf8_lossy(&request_data[..header_end]);
            if let Some(content_length) = parse_content_length(&headers) {
                let body_start = header_end + 4;
                let total_expected = body_start + content_length;
                while request_data.len() < total_expected {
                    let n = stream.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    request_data.extend_from_slice(&buffer[..n]);
                }
            }
            break;
        }
    }

    let (method, path, body, authorization) = parse_http_request(&request_data)?;
    debug!("Request: {method} {path}");

    let response: Result<HttpResponse> = if method == "OPTIONS" {
        Ok(HttpResponse {
            status: 204,
            content_type: "text/plain".into(),
            body: Vec::new(),
        })
    } else if method == "GET" && path == "/health" {
        Ok(HttpResponse {
            status: 200,
            content_type: "text/plain".into(),
            body: b"ok".to_vec(),
        })
    } else {
        if let Some(expected) = token.as_ref() {
            let supplied = authorization
                .as_deref()
                .and_then(|h| h.strip_prefix("Bearer "))
                .unwrap_or_default();
            if supplied != expected {
                warn!("Rejected unauthenticated request to {path}");
                return write_response(
                    &mut stream,
                    &HttpResponse {
                        status: 401,
                        content_type: "text/plain".into(),
                        body: b"Unauthorized".to_vec(),
                    },
                );
            }
        }

        match (method.as_str(), path.as_str()) {
            ("POST", "/v2/pipeline") => handle_pipeline(&conn, &body),
            ("POST", "/pull-updates") => handle_pull_updates(&conn, &body),
            _ => {
                debug!("Unknown endpoint: {method} {path}");
                Ok(HttpResponse {
                    status: 404,
                    content_type: "text/plain".into(),
                    body: b"Not Found".to_vec(),
                })
            }
        }
    };

    let http_response = match response {
        Ok(resp) => resp,
        Err(e) => {
            warn!("Request error: {e}");
            HttpResponse {
                status: 500,
                content_type: "text/plain".into(),
                body: format!("Internal Server Error: {e}").into_bytes(),
            }
        }
    };

    write_response(&mut stream, &http_response)
}

fn write_response(stream: &mut TcpStream, resp: &HttpResponse) -> Result<()> {
    let response_bytes = format_http_response(resp);
    stream.write_all(&response_bytes)?;
    stream.flush()?;
    Ok(())
}

fn handle_pipeline(conn: &Arc<Mutex<Arc<Connection>>>, body: &[u8]) -> Result<HttpResponse> {
    let req: PipelineReqBody = serde_json::from_slice(body)
        .map_err(|e| anyhow!("Failed to parse pipeline request: {e}"))?;

    let conn = conn.lock().unwrap();

    let mut results = Vec::new();

    for request in req.requests {
        let result = match request {
            StreamRequest::Execute(exec_req) => execute_statement(&conn, &exec_req),
            StreamRequest::Batch(batch_req) => execute_batch(&conn, &batch_req),
            StreamRequest::None => StreamResult::Error {
                error: Error {
                    message: "Unknown request type".into(),
                    code: "UNKNOWN".into(),
                },
            },
        };
        results.push(result);
    }

    let resp = PipelineRespBody {
        baton: req.baton,
        base_url: None,
        results,
    };

    Ok(HttpResponse {
        status: 200,
        content_type: "application/json".into(),
        body: serde_json::to_vec(&resp)?,
    })
}

fn execute_statement(conn: &Arc<Connection>, req: &ExecuteStreamReq) -> StreamResult {
    let sql = match &req.stmt.sql {
        Some(s) => s.clone(),
        None => {
            return StreamResult::Error {
                error: Error {
                    message: "No SQL provided".into(),
                    code: "NO_SQL".into(),
                },
            };
        }
    };

    debug!("Executing SQL: {sql}");

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to prepare statement: {e}");
            return StreamResult::Error {
                error: Error {
                    message: e.to_string(),
                    code: "PREPARE_ERROR".into(),
                },
            };
        }
    };

    for (i, arg) in req.stmt.args.iter().enumerate() {
        let core_value = convert_value_to_core(arg);
        if let Err(e) = stmt.bind_at(std::num::NonZero::new(i + 1).unwrap(), core_value) {
            warn!("Failed to bind argument {i}: {e}");
            return StreamResult::Error {
                error: Error {
                    message: e.to_string(),
                    code: "BIND_ERROR".into(),
                },
            };
        }
    }

    let want_rows = req.stmt.want_rows.unwrap_or(true);

    if want_rows {
        match stmt.run_collect_rows() {
            Ok(rows) => {
                let cols: Vec<Col> = (0..stmt.num_columns())
                    .map(|i| Col {
                        name: Some(stmt.get_column_name(i).to_string()),
                        decltype: stmt.get_column_decltype(i),
                    })
                    .collect();

                let result_rows: Vec<Row> = rows
                    .into_iter()
                    .map(|row| Row {
                        values: row.into_iter().map(convert_core_to_value).collect(),
                    })
                    .collect();

                StreamResult::Ok {
                    response: StreamResponse::Execute(ExecuteStreamResp {
                        result: StmtResult {
                            cols,
                            rows: result_rows,
                            affected_row_count: 0,
                            last_insert_rowid: None,
                            replication_index: None,
                            rows_read: 0,
                            rows_written: 0,
                            query_duration_ms: 0.0,
                        },
                    }),
                }
            }
            Err(e) => {
                warn!("Failed to execute statement: {e}");
                StreamResult::Error {
                    error: Error {
                        message: e.to_string(),
                        code: "EXECUTE_ERROR".into(),
                    },
                }
            }
        }
    } else {
        match stmt.run_ignore_rows() {
            Ok(()) => StreamResult::Ok {
                response: StreamResponse::Execute(ExecuteStreamResp {
                    result: StmtResult {
                        cols: vec![],
                        rows: vec![],
                        affected_row_count: 0,
                        last_insert_rowid: None,
                        replication_index: None,
                        rows_read: 0,
                        rows_written: 0,
                        query_duration_ms: 0.0,
                    },
                }),
            },
            Err(e) => {
                warn!("Failed to execute statement: {e}");
                StreamResult::Error {
                    error: Error {
                        message: e.to_string(),
                        code: "EXECUTE_ERROR".into(),
                    },
                }
            }
        }
    }
}

fn execute_batch(conn: &Arc<Connection>, req: &BatchStreamReq) -> StreamResult {
    let batch = &req.batch;
    let mut step_results: Vec<Option<StmtResult>> = Vec::with_capacity(batch.steps.len());
    let mut step_errors: Vec<Option<Error>> = Vec::with_capacity(batch.steps.len());

    for (step_idx, step) in batch.steps.iter().enumerate() {
        let should_execute = match &step.condition {
            None => true,
            Some(cond) => evaluate_condition(cond, &step_results, &step_errors, conn),
        };

        if should_execute {
            let result = execute_batch_step(conn, step);
            match result {
                Ok(stmt_result) => {
                    step_results.push(Some(stmt_result));
                    step_errors.push(None);
                }
                Err(e) => {
                    warn!("Batch step {step_idx} failed: {e}");
                    step_results.push(None);
                    step_errors.push(Some(Error {
                        message: e.to_string(),
                        code: "BATCH_STEP_ERROR".into(),
                    }));
                }
            }
        } else {
            step_results.push(None);
            step_errors.push(None);
        }
    }

    StreamResult::Ok {
        response: StreamResponse::Batch(BatchStreamResp {
            result: BatchResult {
                step_results,
                step_errors,
                replication_index: None,
            },
        }),
    }
}

fn evaluate_condition(
    cond: &BatchCond,
    step_results: &[Option<StmtResult>],
    step_errors: &[Option<Error>],
    conn: &Arc<Connection>,
) -> bool {
    match cond {
        BatchCond::None => true,
        BatchCond::Ok { step } => {
            let idx = *step as usize;
            idx < step_results.len() && step_results[idx].is_some()
        }
        BatchCond::Error { step } => {
            let idx = *step as usize;
            idx < step_errors.len() && step_errors[idx].is_some()
        }
        BatchCond::Not { cond } => !evaluate_condition(cond, step_results, step_errors, conn),
        BatchCond::And(list) => list
            .conds
            .iter()
            .all(|c| evaluate_condition(c, step_results, step_errors, conn)),
        BatchCond::Or(list) => list
            .conds
            .iter()
            .any(|c| evaluate_condition(c, step_results, step_errors, conn)),
        BatchCond::IsAutocommit {} => conn.get_auto_commit(),
    }
}

fn execute_batch_step(conn: &Arc<Connection>, step: &BatchStep) -> Result<StmtResult> {
    let sql = step
        .stmt
        .sql
        .as_ref()
        .ok_or_else(|| anyhow!("No SQL in batch step"))?;

    debug!("Executing batch step SQL: {sql}");

    let mut stmt = conn.prepare(sql)?;

    for (i, arg) in step.stmt.args.iter().enumerate() {
        let core_value = convert_value_to_core(arg);
        stmt.bind_at(std::num::NonZero::new(i + 1).unwrap(), core_value)?;
    }

    let want_rows = step.stmt.want_rows.unwrap_or(true);

    if want_rows {
        let rows = stmt.run_collect_rows()?;

        let cols: Vec<Col> = (0..stmt.num_columns())
            .map(|i| Col {
                name: Some(stmt.get_column_name(i).to_string()),
                decltype: stmt.get_column_decltype(i),
            })
            .collect();

        let result_rows: Vec<Row> = rows
            .into_iter()
            .map(|row| Row {
                values: row.into_iter().map(convert_core_to_value).collect(),
            })
            .collect();

        Ok(StmtResult {
            cols,
            rows: result_rows,
            affected_row_count: 0,
            last_insert_rowid: None,
            replication_index: None,
            rows_read: 0,
            rows_written: 0,
            query_duration_ms: 0.0,
        })
    } else {
        stmt.run_ignore_rows()?;
        Ok(StmtResult {
            cols: vec![],
            rows: vec![],
            affected_row_count: 0,
            last_insert_rowid: None,
            replication_index: None,
            rows_read: 0,
            rows_written: 0,
            query_duration_ms: 0.0,
        })
    }
}

fn handle_pull_updates(conn: &Arc<Mutex<Arc<Connection>>>, body: &[u8]) -> Result<HttpResponse> {
    let req = <PullUpdatesReqProtoBody as Message>::decode(body)
        .map_err(|e| anyhow!("Failed to decode PullUpdatesRequest: {e}"))?;

    debug!(
        "Pull updates request: server_revision={}, client_revision={}",
        req.server_revision, req.client_revision
    );

    let encoding =
        PageUpdatesEncodingReq::try_from(req.encoding).unwrap_or(PageUpdatesEncodingReq::Raw);

    if encoding == PageUpdatesEncodingReq::Zstd {
        return Err(anyhow!("Zstd encoding is not supported"));
    }

    let conn = conn.lock().unwrap();

    let wal_state = conn.wal_state()?;
    debug!("WAL state: max_frame={}", wal_state.max_frame);

    let server_revision: u64 = if req.server_revision.is_empty() {
        wal_state.max_frame
    } else {
        req.server_revision.parse().unwrap_or(wal_state.max_frame)
    };

    let client_revision: u64 = if req.client_revision.is_empty() {
        0
    } else {
        req.client_revision.parse().unwrap_or(0)
    };

    debug!("Using server_revision={server_revision}, client_revision={client_revision}");

    let pages_selector: Option<RoaringBitmap> = if !req.server_pages_selector.is_empty() {
        Some(
            RoaringBitmap::deserialize_from(&req.server_pages_selector[..])
                .map_err(|e| anyhow!("Failed to parse server_pages_selector: {e}"))?,
        )
    } else {
        None
    };

    let mut seen_pages: HashSet<u32> = HashSet::new();
    let mut pages_to_send: Vec<(u32, Vec<u8>)> = Vec::new();

    let frame_size = WAL_FRAME_HEADER_SIZE + PAGE_SIZE;
    let mut frame_buffer = vec![0u8; frame_size];

    debug!(
        "pull-updates: scanning WAL frames {}..={} (client_revision={}, server_revision={})",
        client_revision + 1,
        server_revision,
        client_revision,
        server_revision
    );

    if server_revision > client_revision {
        for frame_no in (client_revision + 1..=server_revision).rev() {
            let frame_info = conn.wal_get_frame(frame_no, &mut frame_buffer)?;

            let page_no = frame_info.page_no;
            // WAL uses 1-based page numbers, sync protocol uses 0-based
            let page_id = page_no - 1;

            if seen_pages.contains(&page_no) {
                continue;
            }

            if let Some(ref selector) = pages_selector
                && !selector.contains(page_id)
            {
                continue;
            }

            seen_pages.insert(page_no);

            let type_byte = frame_buffer[WAL_FRAME_HEADER_SIZE];
            debug!(
                "pull-updates: including page_no={page_no}, frame_no={frame_no}, type_byte={type_byte}, db_size={}",
                frame_info.db_size
            );

            let page_data = frame_buffer[WAL_FRAME_HEADER_SIZE..].to_vec();
            pages_to_send.push((page_id, page_data));
        }
    }

    debug!(
        "pull-updates: sending {} pages, seen_pages={seen_pages:?}",
        pages_to_send.len()
    );
    pages_to_send.reverse();

    let db_size = if wal_state.max_frame > 0 {
        let mut last_frame = vec![0u8; frame_size];
        let last_info = conn.wal_get_frame(wal_state.max_frame, &mut last_frame)?;
        last_info.db_size as u64
    } else {
        0
    };

    let header = PullUpdatesRespProtoBody {
        server_revision: server_revision.to_string(),
        db_size,
        raw_encoding: Some(PageSetRawEncodingProto {}),
        zstd_encoding: None,
        stream_kind: 0,
        apply_mode: 0,
        mvcc_log: None,
        protocol: 1,
    };

    let mut response_body = Vec::new();

    let header_bytes = header.encode_to_vec();
    encode_length_delimited(&mut response_body, &header_bytes);

    for (page_id, page_data) in pages_to_send {
        let page_msg = PageData {
            page_id: page_id as u64,
            encoded_page: Bytes::from(page_data),
        };
        let page_bytes = page_msg.encode_to_vec();
        encode_length_delimited(&mut response_body, &page_bytes);
    }

    debug!(
        "Sending {} bytes in pull-updates response",
        response_body.len()
    );

    Ok(HttpResponse {
        status: 200,
        content_type: "application/protobuf".into(),
        body: response_body,
    })
}

struct HttpResponse {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    (0..data.len().saturating_sub(3)).find(|&i| &data[i..i + 4] == b"\r\n\r\n")
}

fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("content-length:") {
            let value = line.split(':').nth(1)?.trim();
            return value.parse().ok();
        }
    }
    None
}

fn parse_http_request(data: &[u8]) -> Result<(String, String, Vec<u8>, Option<String>)> {
    let header_end = find_header_end(data).ok_or_else(|| anyhow!("Invalid HTTP request"))?;
    let headers = String::from_utf8_lossy(&data[..header_end]);

    let first_line = headers
        .lines()
        .next()
        .ok_or_else(|| anyhow!("Empty request"))?;
    let parts: Vec<&str> = first_line.split_whitespace().collect();

    if parts.len() < 2 {
        return Err(anyhow!("Invalid request line"));
    }

    let mut authorization = None;
    for line in headers.lines().skip(1) {
        if let Some((key, value)) = line.split_once(':')
            && key.trim().eq_ignore_ascii_case("authorization")
        {
            authorization = Some(value.trim().to_string());
        }
    }

    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        data[header_end + 4..].to_vec(),
        authorization,
    ))
}

fn format_http_response(resp: &HttpResponse) -> Vec<u8> {
    let status_text = match resp.status {
        200 => "OK",
        204 => "No Content",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Unknown",
    };

    let header = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: *\r\n\
         Access-Control-Expose-Headers: *\r\n\
         \r\n",
        resp.status,
        status_text,
        resp.content_type,
        resp.body.len()
    );

    let mut result = header.into_bytes();
    result.extend_from_slice(&resp.body);
    result
}

fn encode_length_delimited(output: &mut Vec<u8>, data: &[u8]) {
    let mut len = data.len();
    while len >= 0x80 {
        output.push((len as u8) | 0x80);
        len >>= 7;
    }
    output.push(len as u8);
    output.extend_from_slice(data);
}

fn convert_value_to_core(value: &Value) -> CoreValue {
    match value {
        Value::None | Value::Null => CoreValue::Null,
        Value::Integer { value } => CoreValue::from_i64(*value),
        Value::Float { value } => CoreValue::from_f64(*value),
        Value::Text { value } => CoreValue::Text(turso_core::types::Text {
            value: std::borrow::Cow::Owned(value.clone()),
            subtype: turso_core::types::TextSubtype::Text,
        }),
        Value::Blob { value } => CoreValue::Blob(value.to_vec()),
    }
}

fn convert_core_to_value(value: CoreValue) -> Value {
    match value {
        CoreValue::Null => Value::Null,
        CoreValue::Numeric(turso_core::Numeric::Integer(v)) => Value::Integer { value: v },
        CoreValue::Numeric(turso_core::Numeric::Float(v)) => Value::Float {
            value: f64::from(v),
        },
        CoreValue::Text(t) => Value::Text {
            value: t.value.to_string(),
        },
        CoreValue::Blob(b) => Value::Blob {
            value: Bytes::from(b),
        },
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,sync_hub=debug".to_string()),
        )
        .init();

    let listen = std::env::var("HUB_LISTEN").unwrap_or_else(|_| "127.0.0.1:4333".to_string());
    let db_path = std::env::var("HUB_DB_PATH").unwrap_or_else(|_| "hub.sqld".to_string());
    let token = std::env::var("HUB_TOKEN").ok();

    let io: Arc<dyn turso_core::io::IO> = Arc::new(turso_core::io::PlatformIO::new()?);

    if !std::path::Path::new(&db_path).exists() {
        std::fs::write(&db_path, []).map_err(|e| anyhow!("failed to create db file: {e}"))?;
    }

    let db = turso_core::Database::open_file(io, &db_path)?;
    let conn = db.connect()?;

    info!("Sync hub db path: {db_path}");

    SyncHub::new(listen, conn, token).run()
}
