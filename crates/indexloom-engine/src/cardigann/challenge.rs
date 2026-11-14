//! Independently implemented manual login protocol. No remote HTML, cookies,
//! form actions, hidden fields or image URLs cross the engine/API boundary.
use super::{
    BTreeMap, CardigannExecutor, Duration, ElementRef, EngineError, ExecutionResponse, HeaderName,
    Html, Instant, MAX_REDIRECTS, Mapping, Method, Origin, RequestSpec, Selector, Serialize, Url,
    YamlValue, apply_filters, check_error_selectors, key, mapping, render_headers, render_inputs,
    render_template, scalar, selector_matches,
};
use std::fmt::Write;

const TTL: Duration = Duration::from_secs(300);
const IMAGE_LIMIT: usize = 256 * 1024;
const MAX_FIELDS: usize = 16;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginField {
    pub name: String,
    pub label: String,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginChallengeDescriptor {
    pub id: String,
    pub expires_in_seconds: u64,
    pub fields: Vec<LoginField>,
    pub prompt: Option<String>,
    pub has_image: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum LoginProgress {
    Complete,
    Pending { challenge: LoginChallengeDescriptor },
}

#[derive(Default)]
pub(super) struct InteractiveState {
    pub(super) pending: Option<PendingLogin>,
    window: Option<Instant>,
    attempts: u8,
    image_reads: u8,
}

impl std::fmt::Debug for InteractiveState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InteractiveState(<redacted>)")
    }
}

impl InteractiveState {
    pub(super) fn prune(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.started.elapsed() >= TTL)
        {
            self.pending = None;
        }
    }

    fn budget(&mut self, image: bool) -> Result<(), EngineError> {
        if self
            .window
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(60))
        {
            self.window = Some(Instant::now());
            self.attempts = 0;
            self.image_reads = 0;
        }
        let (count, limit) = if image {
            (&mut self.image_reads, 30)
        } else {
            (&mut self.attempts, 8)
        };
        if *count >= limit {
            return Err(EngineError::LoginChallengeRateLimited);
        }
        *count += 1;
        Ok(())
    }
}

pub(super) struct PendingLogin {
    id: String,
    owner: String,
    started: Instant,
    step: u8,
    request: RequestSpec,
    fields: Vec<LoginField>,
    prompt: Option<String>,
    image: Option<(String, Vec<u8>)>,
}

impl PendingLogin {
    fn descriptor(&self) -> LoginChallengeDescriptor {
        LoginChallengeDescriptor {
            id: self.id.clone(),
            expires_in_seconds: TTL.saturating_sub(self.started.elapsed()).as_secs(),
            fields: self.fields.clone(),
            prompt: self.prompt.clone(),
            has_image: self.image.is_some(),
        }
    }
}

impl CardigannExecutor {
    /// Starts (or retrieves) a user-owned challenge in this indexer's cookie session.
    /// The caller must supply an authenticated actor identifier, never user input.
    ///
    /// # Errors
    /// Returns a bounded challenge, authentication or transport failure.
    pub async fn begin_interactive_login(&self, owner: &str) -> Result<LoginProgress, EngineError> {
        let mut state = self.auth_state.lock().await;
        state.interactive.prune();
        if state.revoked {
            return Err(EngineError::LoginChallengeUnavailable);
        }
        state.interactive.budget(false)?;
        if let Some(pending) = &state.interactive.pending {
            return if pending.owner == owner {
                Ok(LoginProgress::Pending {
                    challenge: pending.descriptor(),
                })
            } else {
                Err(EngineError::LoginChallengeUnavailable)
            };
        }
        state.last_success = None;
        let auth = self.login_mapping()?;
        if self.configured_cookie {
            self.authenticate(auth, false).await?;
            state.last_success = Some(Instant::now());
            state.cookie_expiry = self.engine.cookies.earliest_expiry();
            self.persist_session().await?;
            return Ok(LoginProgress::Complete);
        }
        if auth.get(key("method")).and_then(scalar).as_deref() != Some("form") {
            return Err(EngineError::UnsupportedAuthentication(
                "manual challenges require a form login".into(),
            ));
        }
        let render = self.context.for_search("", BTreeMap::new());
        let url = self.render_url(
            &auth.get(key("path")).and_then(scalar).unwrap_or_default(),
            &render,
        )?;
        let page = self
            .interactive_request(RequestSpec {
                method: Method::GET,
                url,
                headers: render_headers(auth.get(key("headers")), &render)?,
                inputs: vec![],
                raw_query: None,
            })
            .await?;
        let mut pending = self
            .prepare_interactive(auth, &page, true, owner, Instant::now(), 0)
            .await?;
        // Ordinary form logins can enter a second page before asking for a code.
        if pending.fields.is_empty() {
            let response = self.interactive_request(pending.request).await?;
            check_error_selectors(auth.get(key("error")), &response.body)?;
            if let Some(next) = self
                .continuation(auth, &response, owner, pending.started, 1)
                .await?
            {
                pending = next;
            } else {
                self.verify_interactive(auth, &response).await?;
                state.last_success = Some(Instant::now());
                state.cookie_expiry = self.engine.cookies.earliest_expiry();
                self.persist_session().await?;
                return Ok(LoginProgress::Complete);
            }
        }
        let challenge = pending.descriptor();
        state.interactive.pending = Some(pending);
        Ok(LoginProgress::Pending { challenge })
    }

    /// Returns only the already fetched bounded raster image. No URL parameter.
    ///
    /// # Errors
    /// Returns an expired/unauthorized challenge or request budget failure.
    pub async fn login_challenge_image(
        &self,
        owner: &str,
        id: &str,
    ) -> Result<(String, Vec<u8>), EngineError> {
        let mut state = self.auth_state.lock().await;
        state.interactive.prune();
        if state.revoked {
            return Err(EngineError::LoginChallengeUnavailable);
        }
        state.interactive.budget(true)?;
        state
            .interactive
            .pending
            .as_ref()
            .filter(|p| p.owner == owner && p.id == id)
            .and_then(|p| p.image.clone())
            .ok_or(EngineError::LoginChallengeUnavailable)
    }

    /// Consumes the token before any network request. Wrong/failed responses and
    /// malformed submissions cannot be retried against the same tracker nonce.
    ///
    /// # Errors
    /// Returns an invalid/expired challenge, rejected credentials or transport error.
    pub async fn submit_login_challenge(
        &self,
        owner: &str,
        id: &str,
        answers: BTreeMap<String, String>,
    ) -> Result<LoginProgress, EngineError> {
        let mut state = self.auth_state.lock().await;
        state.interactive.prune();
        if state.revoked {
            return Err(EngineError::LoginChallengeUnavailable);
        }
        state.interactive.budget(false)?;
        if !state
            .interactive
            .pending
            .as_ref()
            .is_some_and(|p| p.owner == owner && p.id == id)
        {
            return Err(EngineError::LoginChallengeUnavailable);
        }
        let mut pending = state
            .interactive
            .pending
            .take()
            .ok_or(EngineError::LoginChallengeUnavailable)?;
        if answers.len() != pending.fields.len()
            || pending.fields.iter().any(|f| {
                answers.get(&f.name).is_none_or(|v| {
                    v.trim().is_empty() || v.len() > 512 || v.chars().any(char::is_control)
                })
            })
        {
            return Err(EngineError::InvalidLoginChallenge);
        }
        for (name, value) in answers {
            pending.request.inputs.retain(|(n, _)| n != &name);
            pending.request.inputs.push((name, value));
        }
        let auth = self.login_mapping()?;
        let response = self.interactive_request(pending.request).await?;
        check_error_selectors(auth.get(key("error")), &response.body)?;
        // A repeated challenge is a rejected answer. Detect it before reading
        // images or following the returned form, so a rejected nonce never
        // starts another remote request or reuses any submitted value.
        let repeated_field = {
            let document = Html::parse_document(&response.body);
            let selector =
                Selector::parse("input[name]").map_err(|_| EngineError::InvalidLoginChallenge)?;
            document.select(&selector).any(|input| {
                input
                    .value()
                    .attr("name")
                    .is_some_and(|name| pending.fields.iter().any(|field| field.name == name))
            })
        };
        if repeated_field {
            return Err(EngineError::AuthenticationRejected);
        }
        // Reappearance of the submitted challenge is rejection, not an endless
        // replacement CAPTCHA. A distinct code page may advance at most 4 times.
        if let Some(next) = self
            .continuation(auth, &response, owner, pending.started, pending.step + 1)
            .await?
        {
            if next
                .fields
                .iter()
                .any(|f| pending.fields.iter().any(|old| old.name == f.name))
            {
                return Err(EngineError::AuthenticationRejected);
            }
            let challenge = next.descriptor();
            state.interactive.pending = Some(next);
            return Ok(LoginProgress::Pending { challenge });
        }
        self.verify_interactive(auth, &response).await?;
        state.last_success = Some(Instant::now());
        state.cookie_expiry = self.engine.cookies.earliest_expiry();
        self.persist_session().await?;
        Ok(LoginProgress::Complete)
    }

    /// Revokes in-flight and cached runtimes when credentials/routing change.
    pub async fn invalidate_login(&self) {
        let mut state = self.auth_state.lock().await;
        state.revoked = true;
        state.last_success = None;
        state.interactive.pending = None;
    }

    fn login_mapping(&self) -> Result<&Mapping, EngineError> {
        self.definition
            .spec
            .implementation
            .authentication
            .as_ref()
            .and_then(mapping)
            .ok_or(EngineError::InvalidLoginChallenge)
    }

    async fn continuation(
        &self,
        auth: &Mapping,
        page: &ExecutionResponse,
        owner: &str,
        started: Instant,
        step: u8,
    ) -> Result<Option<PendingLogin>, EngineError> {
        if step > 4 || started.elapsed() >= TTL {
            return Err(EngineError::LoginChallengeUnavailable);
        }
        if browser_challenge(&page.body) {
            return Err(EngineError::BrowserLoginRequired);
        }
        // Only an actual challenge form qualifies as a continuation. No arbitrary
        // navigation or tracker-supplied script is executed.
        if !has_challenge_form(auth, &page.body)? {
            return Ok(None);
        }
        Ok(Some(
            self.prepare_interactive(auth, page, false, owner, started, step)
                .await?,
        ))
    }

    async fn verify_interactive(
        &self,
        auth: &Mapping,
        page: &ExecutionResponse,
    ) -> Result<(), EngineError> {
        check_error_selectors(auth.get(key("error")), &page.body)?;
        let test = auth
            .get(key("test"))
            .and_then(mapping)
            .ok_or(EngineError::InvalidLoginChallenge)?;
        let selector = test
            .get(key("selector"))
            .and_then(scalar)
            .ok_or(EngineError::InvalidLoginChallenge)?;
        let render = self.context.for_search("", BTreeMap::new());
        let response = self
            .interactive_request(RequestSpec {
                method: Method::GET,
                url: self.render_url(
                    &test.get(key("path")).and_then(scalar).unwrap_or_default(),
                    &render,
                )?,
                headers: render_headers(auth.get(key("headers")), &render)?,
                inputs: vec![],
                raw_query: None,
            })
            .await?;
        if !selector_matches(&response.body, &selector)? {
            return Err(EngineError::AuthenticationRejected);
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Bounded challenge-form extraction keeps validated selectors and nonce handling together."
    )]
    async fn prepare_interactive(
        &self,
        auth: &Mapping,
        page: &ExecutionResponse,
        initial: bool,
        owner: &str,
        started: Instant,
        step: u8,
    ) -> Result<PendingLogin, EngineError> {
        if browser_challenge(&page.body) {
            return Err(EngineError::BrowserLoginRequired);
        }
        let render = self.context.for_search("", BTreeMap::new());
        let mut headers = render_headers(auth.get(key("headers")), &render)?;
        headers.retain(|(name, _)| name != reqwest::header::REFERER);
        headers.push((reqwest::header::REFERER, page.final_url.to_string()));
        let (target, values, fields, prompt, image_url) = {
            let doc = Html::parse_document(&page.body);
            let form_selector = css(if initial {
                auth.get(key("form"))
                    .and_then(YamlValue::as_str)
                    .unwrap_or("form")
            } else {
                "form"
            })?;
            let form = doc
                .select(&form_selector)
                .find(|f| initial || form_has_challenge(auth, *f))
                .ok_or(EngineError::AuthenticationFormMissing)?;
            if form
                .value()
                .attr("enctype")
                .is_some_and(|e| e != "application/x-www-form-urlencoded")
            {
                return Err(EngineError::InvalidLoginChallenge);
            }
            let action = if initial {
                auth.get(key("submitpath"))
                    .and_then(YamlValue::as_str)
                    .map(|s| render_template(s, &render))
                    .transpose()?
            } else {
                None
            };
            let target = self.resolve_url(
                &page.final_url,
                action
                    .as_deref()
                    .unwrap_or_else(|| form.value().attr("action").unwrap_or("")),
            )?;
            same_origin(&page.final_url, &target)?;
            let mut values = BTreeMap::new();
            let mut fields = Vec::new();
            for input in form.select(&css("input[name]")?) {
                if input.value().attr("disabled").is_some() {
                    continue;
                }
                let name = input.value().attr("name").unwrap_or_default();
                let kind = input
                    .value()
                    .attr("type")
                    .unwrap_or("text")
                    .to_ascii_lowercase();
                if matches!(kind.as_str(), "submit" | "button" | "reset" | "file") {
                    continue;
                }
                if matches!(kind.as_str(), "checkbox" | "radio")
                    && input.value().attr("checked").is_none()
                {
                    continue;
                }
                values.insert(
                    name.to_owned(),
                    input.value().attr("value").unwrap_or_default().to_owned(),
                );
                if kind != "hidden" && code_input(input) {
                    fields.push(LoginField {
                        name: name.to_owned(),
                        label: code_label(name).into(),
                        kind: "code".into(),
                    });
                }
            }
            if initial {
                for (name, value) in render_inputs(auth.get(key("inputs")), &render, true)? {
                    let name = if auth
                        .get(key("selectors"))
                        .and_then(YamlValue::as_bool)
                        .unwrap_or(false)
                    {
                        doc.select(&css(&name)?)
                            .next()
                            .and_then(|e| e.value().attr("name"))
                            .ok_or(EngineError::InvalidLoginChallenge)?
                            .to_owned()
                    } else {
                        name
                    };
                    values.insert(name, value);
                }
                for section in ["selectorinputs", "getselectorinputs"] {
                    if let Some(inputs) = auth.get(key(section)).and_then(mapping) {
                        for (name, spec) in inputs {
                            let name = name.clone();
                            let spec = mapping(spec).ok_or(EngineError::InvalidLoginChallenge)?;
                            let selector = spec
                                .get(key("selector"))
                                .and_then(scalar)
                                .ok_or(EngineError::InvalidLoginChallenge)?;
                            let Some(element) = doc.select(&css(&selector)?).next() else {
                                if spec.get(key("optional")).and_then(YamlValue::as_bool)
                                    == Some(true)
                                {
                                    continue;
                                }
                                return Err(EngineError::InvalidLoginChallenge);
                            };
                            let value =
                                if let Some(attr) = spec.get(key("attribute")).and_then(scalar) {
                                    element.value().attr(&attr).unwrap_or_default().to_owned()
                                } else {
                                    element.text().collect::<String>()
                                };
                            let value = if let Some(filters) = spec.get(key("filters")) {
                                apply_filters(value, filters)?
                            } else {
                                value
                            };
                            values.insert(
                                if section == "getselectorinputs" {
                                    format!("__query__{name}")
                                } else {
                                    name
                                },
                                value,
                            );
                        }
                    }
                }
            }
            let mut prompt = None;
            let mut image_url = None;
            if let Some(captcha) = auth.get(key("captcha")).and_then(mapping) {
                let kind = captcha
                    .get(key("type"))
                    .and_then(YamlValue::as_str)
                    .unwrap_or("image");
                if !matches!(kind, "image" | "text") {
                    return Err(EngineError::BrowserLoginRequired);
                }
                let selector = captcha
                    .get(key("selector"))
                    .and_then(scalar)
                    .ok_or(EngineError::InvalidLoginChallenge)?;
                if let Some(element) = doc.select(&css(&selector)?).next() {
                    let mut name = captcha
                        .get(key("input"))
                        .and_then(scalar)
                        .ok_or(EngineError::InvalidLoginChallenge)?;
                    if auth.get(key("selectors")).and_then(YamlValue::as_bool) == Some(true) {
                        let updated = doc
                            .select(&css(&name)?)
                            .next()
                            .and_then(|e| e.value().attr("name"))
                            .ok_or(EngineError::InvalidLoginChallenge)?;
                        updated.clone_into(&mut name);
                    }
                    fields.retain(|f| f.name != name);
                    fields.push(LoginField {
                        name,
                        label: "CAPTCHA answer".into(),
                        kind: "text".into(),
                    });
                    if kind == "text" {
                        prompt = Some(
                            element
                                .text()
                                .collect::<String>()
                                .chars()
                                .take(500)
                                .collect(),
                        );
                    } else {
                        let url = self.resolve_url(
                            &page.final_url,
                            element
                                .value()
                                .attr("src")
                                .ok_or(EngineError::InvalidLoginChallenge)?,
                        )?;
                        same_origin(&page.final_url, &url)?;
                        image_url = Some(url);
                    }
                }
            }
            // Explicit custom interactive fields allow site-specific PIN/input
            // names without guessing their values or storing them in config.
            if let Some(items) = auth.get(key("interactive")).and_then(YamlValue::as_array) {
                for item in items {
                    let item = mapping(item).ok_or(EngineError::InvalidLoginChallenge)?;
                    let selector = item
                        .get(key("selector"))
                        .and_then(scalar)
                        .ok_or(EngineError::InvalidLoginChallenge)?;
                    if let Some(input) = form.select(&css(&selector)?).next() {
                        let name = input
                            .value()
                            .attr("name")
                            .ok_or(EngineError::InvalidLoginChallenge)?
                            .to_owned();
                        fields.retain(|f| f.name != name);
                        fields.push(LoginField {
                            name,
                            label: item
                                .get(key("label"))
                                .and_then(scalar)
                                .unwrap_or_else(|| "Tracker response".into())
                                .chars()
                                .take(100)
                                .collect(),
                            kind: "text".into(),
                        });
                    }
                }
            }
            if values.len() > 128
                || values.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() > 65536
                || fields.len() > MAX_FIELDS
                || fields.iter().any(|f| f.name.len() > 128)
            {
                return Err(EngineError::InvalidLoginChallenge);
            }
            (target, values, fields, prompt, image_url)
        };
        let image = if let Some(url) = image_url {
            Some(
                self.fetch_challenge_image(url, &page.final_url, &headers)
                    .await?,
            )
        } else {
            None
        };
        let mut target = target;
        let mut inputs = Vec::new();
        for (name, value) in values {
            if let Some(name) = name.strip_prefix("__query__") {
                target.query_pairs_mut().append_pair(name, &value);
            } else {
                inputs.push((name, value));
            }
        }
        Ok(PendingLogin {
            id: challenge_id()?,
            owner: owner.to_owned(),
            started,
            step,
            request: RequestSpec {
                method: Method::POST,
                url: target,
                headers,
                inputs,
                raw_query: None,
            },
            fields,
            prompt,
            image,
        })
    }

    async fn interactive_request(
        &self,
        spec: RequestSpec,
    ) -> Result<ExecutionResponse, EngineError> {
        let origin = spec.url.clone();
        let mut url = spec.url;
        let mut method = spec.method;
        for _ in 0..=MAX_REDIRECTS {
            self.ensure_allowed(&url)?;
            same_origin(&origin, &url)?;
            let mut request = self
                .engine
                .request(method.clone(), url.clone(), &self.profile);
            for (name, value) in &spec.headers {
                request = request.header(name, value);
            }
            if method == Method::POST {
                request = request.form(&spec.inputs);
            }
            let mut response = request.send().await?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or(EngineError::RedirectWithoutLocation)?;
                url = self.resolve_url(&url, location)?;
                if matches!(response.status().as_u16(), 301..=303) {
                    method = Method::GET;
                }
                continue;
            }
            if !response.status().is_success() {
                return Err(EngineError::AuthenticationRejected);
            }
            if response.content_length().is_some_and(|n| n > 1024 * 1024) {
                return Err(EngineError::ResponseTooLarge);
            }
            let status = response.status();
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
                    return Err(EngineError::ResponseTooLarge);
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(ExecutionResponse {
                status,
                body: String::from_utf8_lossy(&bytes).into_owned(),
                final_url: url,
                response_type: "html".into(),
            });
        }
        Err(EngineError::TooManyRedirects)
    }

    async fn fetch_challenge_image(
        &self,
        mut url: Url,
        page: &Url,
        headers: &[(HeaderName, String)],
    ) -> Result<(String, Vec<u8>), EngineError> {
        for _ in 0..=MAX_REDIRECTS {
            self.ensure_allowed(&url)?;
            same_origin(page, &url)?;
            let mut request = self.engine.request(Method::GET, url.clone(), &self.profile);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let mut response = request.send().await?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or(EngineError::RedirectWithoutLocation)?;
                url = self.resolve_url(&url, location)?;
                continue;
            }
            if !response.status().is_success() {
                return Err(EngineError::AuthenticationRejected);
            }
            let mime = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if !matches!(
                mime.as_str(),
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            ) || response
                .content_length()
                .is_some_and(|n| n > IMAGE_LIMIT as u64)
            {
                return Err(EngineError::InvalidLoginChallenge);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if bytes.len().saturating_add(chunk.len()) > IMAGE_LIMIT {
                    return Err(EngineError::InvalidLoginChallenge);
                }
                bytes.extend_from_slice(&chunk);
            }
            let valid = match mime.as_str() {
                "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
                "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
                "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
                _ => false,
            };
            if !valid {
                return Err(EngineError::InvalidLoginChallenge);
            }
            return Ok((mime, bytes));
        }
        Err(EngineError::TooManyRedirects)
    }
}

fn css(value: &str) -> Result<Selector, EngineError> {
    Selector::parse(value).map_err(|_| EngineError::InvalidLoginChallenge)
}
fn same_origin(page: &Url, target: &Url) -> Result<(), EngineError> {
    if Origin::from_url(page) != Origin::from_url(target)
        || !matches!(target.scheme(), "https" | "http")
        || !target.username().is_empty()
        || target.password().is_some()
    {
        return Err(EngineError::InvalidLoginChallenge);
    }
    Ok(())
}
fn browser_challenge(body: &str) -> bool {
    [
        "g-recaptcha",
        "h-captcha",
        "cf-turnstile",
        "challenges.cloudflare.com",
        "recaptcha/api",
        "hcaptcha.com/1/api",
    ]
    .iter()
    .any(|s| body.contains(s))
}
fn code_input(input: ElementRef<'_>) -> bool {
    let name = input
        .value()
        .attr("name")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace(['-', '_'], "");
    input.value().attr("autocomplete") == Some("one-time-code")
        || matches!(
            name.as_str(),
            "pin"
                | "pincode"
                | "otp"
                | "otpcode"
                | "totp"
                | "totpcode"
                | "2fa"
                | "2facode"
                | "twofactor"
                | "twofactorcode"
                | "twostepcode"
                | "authcode"
                | "verificationcode"
        )
}
fn code_label(name: &str) -> &'static str {
    if name.to_ascii_lowercase().contains("pin") {
        "Tracker PIN"
    } else {
        "Two-factor code"
    }
}
fn form_has_code(form: ElementRef<'_>) -> bool {
    form.select(
        &Selector::parse("input[name]:not([type=hidden]):not([disabled])").expect("static CSS"),
    )
    .any(code_input)
}
fn captcha_present(auth: &Mapping, form: ElementRef<'_>) -> bool {
    auth.get(key("captcha"))
        .and_then(mapping)
        .and_then(|c| c.get(key("selector")))
        .and_then(YamlValue::as_str)
        .and_then(|s| Selector::parse(s).ok())
        .is_some_and(|s| form.select(&s).next().is_some())
}
fn has_challenge_form(auth: &Mapping, body: &str) -> Result<bool, EngineError> {
    let doc = Html::parse_document(body);
    for form in doc.select(&css("form")?) {
        if form_has_challenge(auth, form) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn form_has_challenge(auth: &Mapping, form: ElementRef<'_>) -> bool {
    if form_has_code(form) || captcha_present(auth, form) {
        return true;
    }
    if let Some(items) = auth.get(key("interactive")).and_then(YamlValue::as_array) {
        for item in items {
            if let Some(selector) = mapping(item)
                .and_then(|m| m.get(key("selector")))
                .and_then(YamlValue::as_str)
                && Selector::parse(selector).is_ok_and(|s| form.select(&s).next().is_some())
            {
                return true;
            }
        }
    }
    false
}

pub(super) fn requires_interactive(auth: &Mapping, body: &str) -> Result<bool, EngineError> {
    if browser_challenge(body) {
        return Err(EngineError::BrowserLoginRequired);
    }
    has_challenge_form(auth, body)
}

#[cfg(test)]
mod tests;

fn challenge_id() -> Result<String, EngineError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| EngineError::InvalidLoginChallenge)?;
    let mut result = String::with_capacity(64);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("hex formatting into String never fails");
    }
    Ok(result)
}
