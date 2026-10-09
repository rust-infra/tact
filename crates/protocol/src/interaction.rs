use crate::RequestId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionRequest {
    Permission {
        request_id: RequestId,
        capability: String,
        reason: String,
    },
    Select {
        request_id: RequestId,
        prompt: String,
        options: Vec<String>,
    },
    MultiSelect {
        request_id: RequestId,
        prompt: String,
        options: Vec<String>,
    },
    Confirm {
        request_id: RequestId,
        prompt: String,
    },
    Input {
        request_id: RequestId,
        prompt: String,
        #[serde(default)]
        secret: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionResponse {
    Approved {
        request_id: RequestId,
    },
    Rejected {
        request_id: RequestId,
    },
    Selected {
        request_id: RequestId,
        values: Vec<String>,
    },
    Text {
        request_id: RequestId,
        value: String,
    },
    Cancelled {
        request_id: RequestId,
    },
}
