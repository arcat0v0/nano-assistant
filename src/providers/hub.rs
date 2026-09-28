use crate::hub::HubClient;
use bytes::Bytes;
use futures::StreamExt;
use rig::http_client::{self, HttpClientExt, LazyBody, MultipartForm, Request, Response};
use rig::wasm_compat::WasmCompatSend;
use std::future::Future;

#[derive(Clone, Default)]
pub(super) struct AuthenticatedTransport {
    client: reqwest::Client,
    auth: Option<Authentication>,
}

#[derive(Clone)]
enum Authentication {
    Hub(HubClient),
    Mimo(String),
    AnthropicOAuth(String),
}

impl std::fmt::Debug for AuthenticatedTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mode = match self.auth {
            Some(Authentication::Hub(_)) => "hub",
            Some(Authentication::Mimo(_)) => "mimo",
            Some(Authentication::AnthropicOAuth(_)) => "anthropic-oauth",
            None => "unconfigured",
        };
        f.debug_struct("AuthenticatedTransport")
            .field("mode", &mode)
            .finish()
    }
}

impl AuthenticatedTransport {
    pub(super) fn hub(client: HubClient) -> Self {
        Self {
            auth: Some(Authentication::Hub(client)),
            ..Self::default()
        }
    }

    pub(super) fn mimo(key: String, client: reqwest::Client) -> Self {
        Self {
            client,
            auth: Some(Authentication::Mimo(key)),
        }
    }

    pub(super) fn anthropic_oauth(key: String, client: reqwest::Client) -> Self {
        Self {
            client,
            auth: Some(Authentication::AnthropicOAuth(key)),
        }
    }

    async fn request(
        &self,
        req: Request<()>,
        body: Bytes,
    ) -> http_client::Result<reqwest::Response> {
        let (parts, _) = req.into_parts();
        let response = match self.auth.as_ref() {
            Some(Authentication::Hub(hub)) => {
                let path = parts
                    .uri
                    .path_and_query()
                    .map_or("/", |value| value.as_str());
                if parts.method != reqwest::Method::POST || path != "/v1/chat/completions" {
                    return Err(transport_error(
                        "hub transport only supports POST /v1/chat/completions",
                    ));
                }
                hub.signed_bytes("POST", path, body, true)
                    .await
                    .map_err(|error| transport_error(error.to_string()))?
            }
            Some(Authentication::Mimo(key)) => {
                let mut headers = parts.headers;
                headers.remove(reqwest::header::AUTHORIZATION);
                self.client
                    .request(parts.method, parts.uri.to_string())
                    .headers(headers)
                    .header("api-key", key.as_str())
                    .body(body)
                    .send()
                    .await
                    .map_err(|error| http_client::Error::Instance(Box::new(error)))?
            }
            Some(Authentication::AnthropicOAuth(key)) => {
                let mut headers = parts.headers;
                headers.remove("x-api-key");
                self.client
                    .request(parts.method, parts.uri.to_string())
                    .headers(headers)
                    .bearer_auth(key)
                    .body(body)
                    .send()
                    .await
                    .map_err(|error| http_client::Error::Instance(Box::new(error)))?
            }
            None => return Err(transport_error("authenticated transport is not configured")),
        };

        if !response.status().is_success() {
            let status = response.status();
            let headers = Box::new(response.headers().clone());
            let body = response
                .text()
                .await
                .map_err(|error| http_client::Error::Instance(Box::new(error)))?;
            return Err(http_client::Error::InvalidStatusCodeWithDetails {
                status,
                body,
                headers,
            });
        }
        Ok(response)
    }
}

fn transport_error(message: impl Into<String>) -> http_client::Error {
    http_client::Error::Instance(Box::new(std::io::Error::other(message.into())))
}

impl HttpClientExt for AuthenticatedTransport {
    fn send<T, U>(
        &self,
        req: Request<T>,
    ) -> impl Future<Output = http_client::Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        T: Into<Bytes> + WasmCompatSend,
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        let (parts, body) = req.into_parts();
        let body: Bytes = body.into();
        let transport = self.clone();
        async move {
            let request = Request::from_parts(parts, ());
            let response = transport.request(request, body).await?;
            let mut result = Response::builder().status(response.status());
            if let Some(headers) = result.headers_mut() {
                *headers = response.headers().clone();
            }
            let body: LazyBody<U> = Box::pin(async move {
                response
                    .bytes()
                    .await
                    .map(U::from)
                    .map_err(|error| http_client::Error::Instance(Box::new(error)))
            });
            result.body(body).map_err(Into::into)
        }
    }

    fn send_multipart<U>(
        &self,
        _req: Request<MultipartForm>,
    ) -> impl Future<Output = http_client::Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        async {
            Err(transport_error(
                "authenticated completion transport does not support multipart requests",
            ))
        }
    }

    fn send_streaming<T>(
        &self,
        req: Request<T>,
    ) -> impl Future<Output = http_client::Result<http_client::StreamingResponse>> + WasmCompatSend
    where
        T: Into<Bytes> + WasmCompatSend,
    {
        let (parts, body) = req.into_parts();
        let body: Bytes = body.into();
        let transport = self.clone();
        async move {
            let request = Request::from_parts(parts, ());
            let response = transport.request(request, body).await?;
            let mut result = Response::builder()
                .status(response.status())
                .version(response.version());
            if let Some(headers) = result.headers_mut() {
                *headers = response.headers().clone();
            }
            let stream: http_client::sse::BoxedStream =
                Box::pin(response.bytes_stream().map(|chunk| {
                    chunk.map_err(|error| http_client::Error::Instance(Box::new(error)))
                }));
            result.body(stream).map_err(Into::into)
        }
    }
}
