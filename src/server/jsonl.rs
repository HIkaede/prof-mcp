//! Line-delimited access to the same experimental tool handlers as stdio MCP.

use std::io::{self, BufRead, Read, Write};

use rmcp::model::JsonObject;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    ProfileServer,
    experimental::{MAX_REQUEST_BYTES, invalid_request, request_too_large},
    respond::experimental_too_large,
};
use crate::{config::Config, error::ApiError, output::ResponseBudget};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    tool: String,
    #[serde(default)]
    arguments: JsonObject,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Response {
    Result(Value),
    Error(ApiError),
}

pub async fn run_jsonl(config: Config) -> anyhow::Result<()> {
    let server = ProfileServer::new(config)?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut request = Vec::new();
    loop {
        request.clear();
        let bytes = (&mut input)
            .take((MAX_REQUEST_BYTES + 1) as u64)
            .read_until(b'\n', &mut request)?;
        if bytes == 0 {
            break;
        }
        let result = if bytes > MAX_REQUEST_BYTES {
            if !request.ends_with(b"\n") {
                input.skip_until(b'\n')?;
            }
            Err(request_too_large())
        } else {
            match serde_json::from_slice::<Request>(&request) {
                Ok(request) => {
                    server
                        .experimental_call(&request.tool, request.arguments, || false)
                        .await
                }
                Err(error) => Err(invalid_request(error)),
            }
        };
        let response = match result {
            Ok(value) => Response::Result(value),
            Err(error) => Response::Error(error),
        };
        let response = if ResponseBudget::new().json(&response).is_ok() {
            response
        } else {
            Response::Error(experimental_too_large())
        };
        let bytes = serde_json::to_vec(&response)?;
        if let Err(error) = output
            .write_all(&bytes)
            .and_then(|()| output.write_all(b"\n"))
            .and_then(|()| output.flush())
        {
            if error.kind() == io::ErrorKind::BrokenPipe {
                break;
            }
            return Err(error.into());
        }
    }
    Ok(())
}
