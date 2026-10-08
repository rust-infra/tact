use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use tact_protocol::{PluginRequestEnvelope, PluginResponseEnvelope};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::Instant,
};

pub(crate) struct NodePluginProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

impl NodePluginProcess {
    pub(crate) async fn spawn(program: &Path, args: &[String]) -> Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("start Node plugin process {}", program.display()))?;
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

    pub(crate) async fn request(
        &mut self,
        request: &PluginRequestEnvelope,
        timeout: Duration,
    ) -> Result<PluginResponseEnvelope> {
        let mut encoded = serde_json::to_vec(request).context("encode Node plugin request")?;
        encoded.push(b'\n');
        self.stdin
            .write_all(&encoded)
            .await
            .context("write Node plugin request")?;
        self.stdin
            .flush()
            .await
            .context("flush Node plugin request")?;

        let line = tokio::time::timeout_at(Instant::now() + timeout, self.stdout.next_line())
            .await
            .map_err(|_| anyhow!("Node plugin request timed out"))?
            .context("read Node plugin response")?
            .ok_or_else(|| anyhow!("Node plugin exited before responding"))?;
        let response: PluginResponseEnvelope =
            serde_json::from_str(&line).context("decode Node plugin response")?;
        if response.request_id != request.request_id {
            bail!("Node plugin response request ID does not match request");
        }
        if response.plugin_id != request.plugin_id {
            bail!("Node plugin response plugin ID does not match request");
        }
        if !response
            .protocol_version
            .compatible_with(request.protocol_version)
        {
            bail!("Node plugin response protocol version is incompatible");
        }
        Ok(response)
    }

    pub(crate) async fn wait(&mut self) -> Result<()> {
        self.child
            .wait()
            .await
            .context("wait for Node plugin exit")?;
        Ok(())
    }

    pub(crate) async fn terminate(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
    }
}
