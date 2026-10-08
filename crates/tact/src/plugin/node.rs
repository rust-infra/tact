//! Node.js plugin process transport.

use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use tact_protocol::{PluginRequestEnvelope, PluginResponseEnvelope};

pub struct NodePluginProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

impl NodePluginProcess {
    pub async fn spawn(program: impl AsRef<std::path::Path>, args: &[String]) -> Result<Self> {
        let mut command = Command::new(program.as_ref());
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn().context("start Node plugin process")?;
        let stdin = child
            .stdin
            .take()
            .context("Node plugin stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("Node plugin stdout unavailable")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
        })
    }

    pub async fn send(&mut self, request: &PluginRequestEnvelope) -> Result<()> {
        let mut encoded = serde_json::to_vec(request).context("encode plugin request")?;
        encoded.push(b'\n');
        self.stdin
            .write_all(&encoded)
            .await
            .context("write plugin request")?;
        self.stdin.flush().await.context("flush plugin request")?;
        Ok(())
    }

    pub async fn receive(&mut self) -> Result<PluginResponseEnvelope> {
        let Some(line) = self
            .stdout
            .next_line()
            .await
            .context("read plugin response")?
        else {
            let status = self.child.try_wait().context("check Node plugin status")?;
            bail!("Node plugin exited before responding: {status:?}");
        };
        serde_json::from_str(&line).context("decode plugin response")
    }

    pub async fn request(
        &mut self,
        request: &PluginRequestEnvelope,
    ) -> Result<PluginResponseEnvelope> {
        self.send(request).await?;
        self.receive().await
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.child.kill().await.context("stop Node plugin process")
    }

    pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
        serde_json::to_vec(value).context("encode Node plugin value")
    }

    pub fn decode<T: DeserializeOwned>(value: &[u8]) -> Result<T> {
        serde_json::from_slice(value).context("decode Node plugin value")
    }
}
