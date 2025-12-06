use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde::Deserialize;
use tokio::process::Command;

/// HTTP client using curl for making GitHub API requests
pub struct GithubCurlClient {
    token: String,
}

#[derive(Debug, Deserialize)]
struct GitHubError {
    message: String,
    #[serde(default)]
    #[allow(dead_code)]
    documentation_url: Option<String>,
}

impl GithubCurlClient {
    pub fn new(token: String) -> Self {
        Self { token }
    }

    /// Make a GET request
    pub async fn get(&self, url: &str, accept: &str) -> Result<String> {
        let output = Command::new("curl")
            .args([
                "-s",
                "-w",
                "\n%{http_code}",
                "-H",
                &format!("Authorization: Bearer {}", self.token),
                "-H",
                &format!("Accept: {}", accept),
                "-H",
                "User-Agent: jr-cli",
                url,
            ])
            .output()
            .await
            .context("Failed to execute curl command")?;

        if !output.status.success() {
            bail!(
                "curl command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        self.parse_response(output.stdout)
    }

    /// Make a POST request
    pub async fn post(&self, url: &str, json_data: &str) -> Result<String> {
        let output = Command::new("curl")
            .args([
                "-s",
                "-w",
                "\n%{http_code}",
                "-X",
                "POST",
                "-H",
                &format!("Authorization: Bearer {}", self.token),
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "Content-Type: application/json",
                "-H",
                "User-Agent: jr-cli",
                "-d",
                json_data,
                url,
            ])
            .output()
            .await
            .context("Failed to execute curl command")?;

        if !output.status.success() {
            bail!(
                "curl command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        self.parse_response(output.stdout)
    }

    /// Make a PATCH request
    pub async fn patch(&self, url: &str, json_data: &str) -> Result<String> {
        let output = Command::new("curl")
            .args([
                "-s",
                "-w",
                "\n%{http_code}",
                "-X",
                "PATCH",
                "-H",
                &format!("Authorization: Bearer {}", self.token),
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "Content-Type: application/json",
                "-H",
                "User-Agent: jr-cli",
                "-d",
                json_data,
                url,
            ])
            .output()
            .await
            .context("Failed to execute curl command")?;

        if !output.status.success() {
            bail!(
                "curl command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        self.parse_response(output.stdout)
    }

    /// Make a DELETE request
    pub async fn delete(&self, url: &str) -> Result<()> {
        let output = Command::new("curl")
            .args([
                "-s",
                "-w",
                "\n%{http_code}",
                "-X",
                "DELETE",
                "-H",
                &format!("Authorization: Bearer {}", self.token),
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "User-Agent: jr-cli",
                url,
            ])
            .output()
            .await
            .context("Failed to execute curl command")?;

        if !output.status.success() {
            bail!(
                "curl command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        self.parse_response(output.stdout)?;
        Ok(())
    }

    /// Parse curl response with status code appended
    fn parse_response(&self, stdout: Vec<u8>) -> Result<String> {
        let output_str = String::from_utf8(stdout)?;
        let mut lines: Vec<&str> = output_str.rsplitn(2, '\n').collect();
        lines.reverse();

        let response = lines.first().unwrap_or(&"").to_string();
        let status_code = lines
            .get(1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);

        // Check HTTP status code
        if status_code >= 400 {
            // Try to parse error message from response
            if let Ok(error) = serde_json::from_str::<GitHubError>(&response) {
                bail!("GitHub API error: {}", error.message);
            }
            bail!(
                "GitHub API request failed with status {}: {}",
                status_code,
                response
            );
        }

        Ok(response)
    }
}
