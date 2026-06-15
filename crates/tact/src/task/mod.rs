//! Durable task management.
//!
//! Tasks are persistent work items with status (Pending → InProgress →
//! Completed/Deleted), blocking relationships, and optional owners.
//!
//! - [`TaskManager`] is the core state machine backed by a file store.
//! - [`SharedTaskManager`] wraps it in `Arc<Mutex<…>>` for concurrent
//!   access from tools.
//! - [`TaskRecord`] is the wire format; it supports `blockedBy` / `blocks`
//!   for dependency tracking.
//! - [`render_task_json`] and [`render_task_list`] produce LLM-friendly
//!   textual representations.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use strum::EnumProperty;
use strum_macros::{Display, EnumProperty as EnumPropertyDerive, EnumString};

use crate::store::{CollectionStore, Store, StoreRoot};

/// Task lifecycle states.
///
/// Each state has a visual marker for LLM-friendly list rendering
/// (`[ ]` → `[>]` → `[x]` / `[-]`).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    Display,
    EnumPropertyDerive,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum TaskStatus {
    #[strum(props(marker = "[ ]"))]
    Pending,
    #[strum(props(marker = "[>]"))]
    InProgress,
    #[strum(props(marker = "[x]"))]
    Completed,
    #[strum(props(marker = "[-]"))]
    Deleted,
}

impl TaskStatus {
    pub fn marker(self) -> &'static str {
        self.get_str("marker").unwrap_or("[?]")
    }
}

/// A record of a task in the manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: u64,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: TaskStatus,
    #[serde(rename = "blockedBy", default)]
    pub blocked_by: Vec<u64>,
    #[serde(default)]
    pub blocks: Vec<u64>,
    #[serde(default)]
    pub owner: String,
}

impl TaskRecord {
    /// Creates a new task record.
    pub fn new(id: u64, subject: String, description: Option<String>) -> Self {
        Self {
            id,
            subject,
            description,
            status: TaskStatus::Pending,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            owner: String::new(),
        }
    }
}

/// The index of the next task ID to use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskIndex {
    pub next_id: u64,
}

impl Default for TaskIndex {
    fn default() -> Self {
        Self { next_id: 1 }
    }
}

/// A mutable update to apply to an existing task.
#[derive(Debug, Clone, Default)]
pub struct TaskUpdate {
    pub status: Option<TaskStatus>,
    pub owner: Option<String>,
    pub add_blocked_by: Vec<u64>,
    pub add_blocks: Vec<u64>,
}

/// Core task manager backed by a file-based collection store.
#[derive(Debug)]
pub struct TaskManager {
    tasks: CollectionStore<TaskRecord>,
    index: Store<TaskIndex>,
}

impl TaskManager {
    /// Creates a new task manager from the given store root.
    pub fn new(root: &StoreRoot) -> Result<Self> {
        let manager = Self {
            tasks: root.collection("tasks")?,
            index: root.file("tasks/index.json")?,
        };
        if !manager.index.exists() {
            manager.index.write(&TaskIndex::default())?;
        }
        Ok(manager)
    }

    /// Creates a new task with the given subject and description.
    pub fn create(&mut self, subject: String, description: Option<String>) -> Result<TaskRecord> {
        let mut index = self.index.read().unwrap_or_default();
        let task = TaskRecord::new(index.next_id, subject, description);
        self.tasks.write(&task_key(task.id), &task)?;
        index.next_id += 1;
        self.index.write(&index)?;
        Ok(task)
    }

    /// Gets the task with the given ID.
    pub fn get(&self, task_id: u64) -> Result<TaskRecord> {
        self.tasks
            .read(&task_key(task_id))
            .with_context(|| format!("Task {} not found", task_id))
    }

    /// Updates the task with the given ID using the given update.
    pub fn update(&mut self, task_id: u64, update: TaskUpdate) -> Result<TaskRecord> {
        let mut task = self.get(task_id)?;

        if let Some(owner) = update.owner {
            task.owner = owner;
        }

        if let Some(status) = update.status {
            task.status = status;
            if status == TaskStatus::Completed {
                self.clear_dependency(task_id)?;
            }
        }

        if !update.add_blocked_by.is_empty() {
            merge_unique(&mut task.blocked_by, update.add_blocked_by);
        }

        if !update.add_blocks.is_empty() {
            merge_unique(&mut task.blocks, update.add_blocks.clone());
            for blocked_id in update.add_blocks {
                if let Ok(mut blocked) = self.get(blocked_id)
                    && !blocked.blocked_by.contains(&task_id)
                {
                    blocked.blocked_by.push(task_id);
                    blocked.blocked_by.sort_unstable();
                    self.tasks.write(&task_key(blocked.id), &blocked)?;
                }
            }
        }

        task.blocked_by.sort_unstable();
        task.blocks.sort_unstable();
        self.tasks.write(&task_key(task.id), &task)?;
        Ok(task)
    }

    /// Lists all tasks in the manager.
    pub fn list(&self) -> Result<Vec<TaskRecord>> {
        let mut tasks = self.tasks.list()?;
        tasks.sort_by_key(|task| task.id);
        Ok(tasks)
    }

    /// Deletes the task with the given ID.
    pub fn delete(&mut self, task_id: u64) -> Result<TaskRecord> {
        let mut task = self.get(task_id)?;
        task.status = TaskStatus::Deleted;
        self.tasks.write(&task_key(task.id), &task)?;
        Ok(task)
    }

    /// Clears the dependency of the task with the given ID.
    fn clear_dependency(&self, completed_id: u64) -> Result<()> {
        for mut task in self.list()? {
            if task.blocked_by.contains(&completed_id) {
                task.blocked_by.retain(|id| *id != completed_id);
                self.tasks.write(&task_key(task.id), &task)?;
            }
        }
        Ok(())
    }
}

/// Thread-safe wrapper around [`TaskManager`].
#[derive(Clone, Debug)]
pub struct SharedTaskManager {
    inner: Arc<Mutex<TaskManager>>,
}

impl SharedTaskManager {
    /// Creates a new shared task manager with the given task manager.
    pub fn new(manager: TaskManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(manager)),
        }
    }

    /// Creates a new task in the manager.
    pub fn create(&self, subject: String, description: Option<String>) -> Result<TaskRecord> {
        self.with_manager(|manager| manager.create(subject, description))
    }

    /// Gets a task from the manager.
    pub fn get(&self, task_id: u64) -> Result<TaskRecord> {
        self.with_manager(|manager| manager.get(task_id))
    }

    /// Updates a task in the manager.
    pub fn update(&self, task_id: u64, update: TaskUpdate) -> Result<TaskRecord> {
        self.with_manager(|manager| manager.update(task_id, update))
    }

    /// Lists all tasks in the manager.
    pub fn list(&self) -> Result<Vec<TaskRecord>> {
        self.with_manager(|manager| manager.list())
    }

    /// Deletes a task from the manager.
    pub fn delete(&self, task_id: u64) -> Result<TaskRecord> {
        self.with_manager(|manager| manager.delete(task_id))
    }

    /// Runs a callback with the task manager locked.
    fn with_manager<T>(&self, callback: impl FnOnce(&mut TaskManager) -> Result<T>) -> Result<T> {
        let mut manager = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("task manager lock poisoned"))?;
        callback(&mut manager)
    }
}

impl std::ops::Deref for SharedTaskManager {
    type Target = Arc<Mutex<TaskManager>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Renders a task as JSON.
pub fn render_task_json(task: &TaskRecord) -> Result<String> {
    serde_json::to_string_pretty(task).context("failed to serialize task")
}

/// Renders a list of tasks as a string.
pub fn render_task_list(tasks: Vec<TaskRecord>) -> String {
    if tasks.is_empty() {
        return "No tasks.".to_string();
    }

    tasks
        .into_iter()
        .map(|task| {
            let blocked = if task.blocked_by.is_empty() {
                String::new()
            } else {
                format!(" (blocked by: {:?})", task.blocked_by)
            };
            let owner = if task.owner.is_empty() {
                String::new()
            } else {
                format!(" owner={}", task.owner)
            };
            format!(
                "{} #{}: {}{}{}",
                task.status.marker(),
                task.id,
                task.subject,
                owner,
                blocked
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn task_key(task_id: u64) -> String {
    format!("task_{task_id}")
}

fn merge_unique(target: &mut Vec<u64>, mut additions: Vec<u64>) {
    target.append(&mut additions);
    target.sort_unstable();
    target.dedup();
}
