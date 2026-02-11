use crate::core::AppError;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// Minimal W3C WebDriver client (talks to chromedriver).
// This intentionally implements only the endpoints we need.

const WEB_ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";

#[derive(Clone)]
pub struct WebDriver {
	base: Url,
	session_id: String,
	http: reqwest::Client,
}

#[derive(Clone, Debug, Default)]
pub struct SeleniumOptions {
	pub headless: bool,
	pub chrome_binary_path: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Element {
	id: String,
}

impl WebDriver {
	pub async fn connect(chromedriver_url: &str, headless: bool) -> Result<Self, AppError> {
		Self::connect_with_options(
			chromedriver_url,
			SeleniumOptions {
				headless,
				chrome_binary_path: None,
			},
		)
		.await
	}

	pub async fn connect_with_options(
		chromedriver_url: &str,
		options: SeleniumOptions,
	) -> Result<Self, AppError> {
		let base = Url::parse(chromedriver_url)
			.map_err(|e| AppError::KiteApi(format!("Invalid CHROMEDRIVER_URL: {e}")))?;
		let http = reqwest::Client::new();

		let create = CreateSession {
			capabilities: Capabilities {
				always_match: AlwaysMatch {
					browser_name: "chrome".to_string(),
					chrome_options: ChromeOptions {
						args: chrome_args(options.headless),
						binary: options.chrome_binary_path,
					},
				},
			},
		};

		let url = base
			.join("/session")
			.map_err(|e| AppError::KiteApi(format!("Bad chromedriver base url: {e}")))?;
		let resp = http.post(url).json(&create).send().await?;
		let status = resp.status();
		let text = resp.text().await?;
		if !status.is_success() {
			return Err(AppError::KiteApi(format!(
				"WebDriver create session failed HTTP {status}: {text}"
			)));
		}

		let json: serde_json::Value = serde_json::from_str(&text)?;
		let session_id = extract_session_id(&json).ok_or_else(|| {
			AppError::KiteApi(format!(
				"WebDriver create session: could not find sessionId in response: {text}"
			))
		})?;

		Ok(Self {
			base,
			session_id,
			http,
		})
	}

	pub async fn goto(&self, url: &str) -> Result<(), AppError> {
		let endpoint = self.endpoint("/url")?;
		self.http
			.post(endpoint)
			.json(&serde_json::json!({"url": url}))
			.send()
			.await?
			.error_for_status()?;
		Ok(())
	}

	pub async fn current_url(&self) -> Result<String, AppError> {
		let endpoint = self.endpoint("/url")?;
		let resp: WebDriverResponse<serde_json::Value> = self.http.get(endpoint).send().await?.json().await?;
		Ok(resp.value.as_str().unwrap_or_default().to_string())
	}

	pub async fn find_css(&self, selector: &str) -> Result<Element, AppError> {
		self.find("css selector", selector).await
	}

	pub async fn find_xpath(&self, xpath: &str) -> Result<Element, AppError> {
		self.find("xpath", xpath).await
	}

	pub async fn click(&self, el: &Element) -> Result<(), AppError> {
		let endpoint = self.endpoint(&format!("/element/{}/click", el.id))?;
		self.http.post(endpoint).json(&serde_json::json!({})).send().await?.error_for_status()?;
		Ok(())
	}

	pub async fn clear(&self, el: &Element) -> Result<(), AppError> {
		let endpoint = self.endpoint(&format!("/element/{}/clear", el.id))?;
		let _ = self.http.post(endpoint).json(&serde_json::json!({})).send().await?;
		Ok(())
	}

	pub async fn send_keys(&self, el: &Element, text: &str) -> Result<(), AppError> {
		let endpoint = self.endpoint(&format!("/element/{}/value", el.id))?;
		let payload = serde_json::json!({
			"text": text,
			"value": text.chars().map(|c| c.to_string()).collect::<Vec<_>>()
		});
		self.http.post(endpoint).json(&payload).send().await?.error_for_status()?;
		Ok(())
	}

	pub async fn is_displayed(&self, el: &Element) -> Result<bool, AppError> {
		let endpoint = self.endpoint(&format!("/element/{}/displayed", el.id))?;
		let resp: WebDriverResponse<serde_json::Value> = self.http.get(endpoint).send().await?.json().await?;
		Ok(resp.value.as_bool().unwrap_or(false))
	}

	pub async fn screenshot_base64(&self) -> Result<String, AppError> {
		let endpoint = self.endpoint("/screenshot")?;
		let resp: WebDriverResponse<String> = self.http.get(endpoint).send().await?.json().await?;
		Ok(resp.value)
	}

	pub async fn page_source(&self) -> Result<String, AppError> {
		let endpoint = self.endpoint("/source")?;
		let resp: WebDriverResponse<String> = self.http.get(endpoint).send().await?.json().await?;
		Ok(resp.value)
	}

	pub async fn quit(&self) -> Result<(), AppError> {
		let endpoint = self
			.base
			.join(&format!("/session/{}", self.session_id))
			.map_err(|e| AppError::KiteApi(format!("Bad session quit URL: {e}")))?;
		let _ = self.http.delete(endpoint).send().await;
		Ok(())
	}

	pub async fn wait_for_css(
		&self,
		selector: &str,
		timeout: Duration,
	) -> Result<Element, AppError> {
		let deadline = tokio::time::Instant::now() + timeout;
		loop {
			match self.find_css(selector).await {
				Ok(el) => return Ok(el),
				Err(_) => {
					if tokio::time::Instant::now() >= deadline {
						return Err(AppError::KiteApi(format!(
							"Timeout waiting for element css={selector}"
						)));
					}
					tokio::time::sleep(Duration::from_millis(250)).await;
				}
			}
		}
	}

	pub async fn wait_for_any_css(
		&self,
		selectors: &[&str],
		timeout: Duration,
	) -> Result<Element, AppError> {
		let deadline = tokio::time::Instant::now() + timeout;
		loop {
			for sel in selectors {
				if let Ok(el) = self.find_css(sel).await {
					return Ok(el);
				}
			}
			if tokio::time::Instant::now() >= deadline {
				return Err(AppError::KiteApi(format!(
					"Timeout waiting for any element css={:?}",
					selectors
				)));
			}
			tokio::time::sleep(Duration::from_millis(250)).await;
		}
	}

	pub async fn wait_for_url_contains(&self, needle: &str, timeout: Duration) -> Result<String, AppError> {
		let deadline = tokio::time::Instant::now() + timeout;
		loop {
			let cur = self.current_url().await.unwrap_or_default();
			if cur.contains(needle) {
				return Ok(cur);
			}
			if tokio::time::Instant::now() >= deadline {
				return Err(AppError::KiteApi(format!(
					"Timeout waiting for redirect containing: {needle}. Last URL: {cur}"
				)));
			}
			tokio::time::sleep(Duration::from_millis(300)).await;
		}
	}

	fn endpoint(&self, path: &str) -> Result<Url, AppError> {
		self.base
			.join(&format!("/session/{}{}", self.session_id, path))
			.map_err(|e| AppError::KiteApi(format!("Bad WebDriver URL: {e}")))
	}

	async fn find(&self, using: &str, value: &str) -> Result<Element, AppError> {
		let endpoint = self.endpoint("/element")?;
		let payload = serde_json::json!({"using": using, "value": value});
		let resp = self.http.post(endpoint).json(&payload).send().await?;
		if !resp.status().is_success() {
			return Err(AppError::KiteApi(format!("Element not found: {using}={value}")));
		}
		let resp: WebDriverResponse<serde_json::Value> = resp.json().await?;
		let id = resp
			.value
			.get(WEB_ELEMENT_KEY)
			.and_then(|v| v.as_str())
			.ok_or_else(|| AppError::KiteApi("Malformed element response".to_string()))?
			.to_string();
		Ok(Element { id })
	}
}

fn extract_session_id(v: &serde_json::Value) -> Option<String> {
	// Chromedriver can respond in W3C format:
	//   {"value": {"sessionId": "...", "capabilities": {...}}}
	// or legacy-ish format:
	//   {"sessionId": "...", "value": {...}}
	v.get("value")
		.and_then(|vv| vv.get("sessionId"))
		.and_then(|s| s.as_str())
		.map(|s| s.to_string())
		.or_else(|| v.get("sessionId").and_then(|s| s.as_str()).map(|s| s.to_string()))
}

fn chrome_args(headless: bool) -> Vec<String> {
	let mut args = vec![
		"--no-sandbox".to_string(),
		"--disable-dev-shm-usage".to_string(),
		"--disable-gpu".to_string(),
		"--remote-debugging-pipe".to_string(),
		"--window-size=1920,1080".to_string(),
	];
	if headless {
		args.push("--headless=new".to_string());
	}
	args
}

#[derive(Debug, Serialize)]
struct CreateSession {
	capabilities: Capabilities,
}

#[derive(Debug, Serialize)]
struct Capabilities {
	#[serde(rename = "alwaysMatch")]
	always_match: AlwaysMatch,
}

#[derive(Debug, Serialize)]
struct AlwaysMatch {
	#[serde(rename = "browserName")]
	browser_name: String,
	#[serde(rename = "goog:chromeOptions")]
	chrome_options: ChromeOptions,
}

#[derive(Debug, Serialize)]
struct ChromeOptions {
	args: Vec<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	#[serde(rename = "binary")]
	binary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WebDriverResponse<T> {
	value: T,
}
