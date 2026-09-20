use crate::{
    Error, Result,
    domain::*,
    registry::{Entry, Search},
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
    pub node_id: String,
}
impl Store {
    // Legacy admission cannot supply durable scoped execution authority.
    pub(crate) async fn require_legacy_execution(&self, workspace: Uuid) -> Result<()> {
        let scoped: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM authorization_workspaces WHERE workspace_id=$1)",
        )
        .bind(workspace)
        .fetch_one(&self.pool)
        .await?;
        if scoped {
            return Err(Error::Forbidden);
        }
        Ok(())
    }

    pub async fn connect(url: &str, node_id: String) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(16)
            .connect(url)
            .await?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| Error::External(e.to_string()))?;
        Ok(Self { pool, node_id })
    }

    /// Share the database and connection settings without sharing pool capacity.
    pub async fn isolated_pool(&self) -> Result<Self> {
        let pool = self
            .pool
            .options()
            .clone()
            .connect_with(self.pool.connect_options().as_ref().clone())
            .await?;
        Ok(Self {
            pool,
            node_id: self.node_id.clone(),
        })
    }
    pub async fn event(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: Option<Uuid>,
        kind: &str,
        data: Value,
    ) -> Result<Event> {
        // Sequence allocation and commit order must agree for Last-Event-ID replay.
        sqlx::query("SELECT pg_advisory_xact_lock(71003201)")
            .execute(&mut **tx)
            .await?;
        Ok(sqlx::query_as("INSERT INTO events(id,node_id,workspace_id,kind,data) VALUES($1,$2,$3,$4,$5) RETURNING *")
            .bind(Uuid::new_v4()).bind(&self.node_id).bind(workspace).bind(kind).bind(data).fetch_one(&mut **tx).await?)
    }
    pub async fn emit(&self, workspace: Option<Uuid>, kind: &str, data: Value) -> Result<Event> {
        let mut tx = self.pool.begin().await?;
        let event = self.event(&mut tx, workspace, kind, data).await?;
        tx.commit().await?;
        Ok(event)
    }
    pub async fn create_workspace(&self, title: &str, goal: &str) -> Result<Workspace> {
        let mut tx = self.pool.begin().await?;
        let workspace = self
            .create_workspace_in(&mut tx, Uuid::new_v4(), title, goal)
            .await?;
        tx.commit().await?;
        Ok(workspace)
    }
    pub(crate) async fn create_workspace_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        title: &str,
        goal: &str,
    ) -> Result<Workspace> {
        nonempty(title, "title")?;
        nonempty(goal, "goal")?;
        let w: Workspace =
            sqlx::query_as("INSERT INTO workspaces(id,title,goal) VALUES($1,$2,$3) RETURNING *")
                .bind(id)
                .bind(title)
                .bind(goal)
                .fetch_one(&mut **tx)
                .await?;
        self.event(tx, Some(w.id), "workspace.created", json!(w))
            .await?;
        Ok(w)
    }
    pub async fn workspaces(&self) -> Result<Vec<Workspace>> {
        Ok(
            sqlx::query_as("SELECT * FROM workspaces ORDER BY created_at DESC")
                .fetch_all(&self.pool)
                .await?,
        )
    }
    pub async fn workspace(&self, id: Uuid) -> Result<Workspace> {
        sqlx::query_as("SELECT * FROM workspaces WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound("workspace".into()))
    }
    pub async fn update_state(&self, id: Uuid, revision: i64, state: Value) -> Result<Workspace> {
        let mut tx = self.pool.begin().await?;
        let workspace = self.update_state_in(&mut tx, id, revision, state).await?;
        tx.commit().await?;
        Ok(workspace)
    }
    pub(crate) async fn update_state_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        revision: i64,
        state: Value,
    ) -> Result<Workspace> {
        if !state.is_object() {
            return Err(Error::Invalid("workspace state must be an object".into()));
        }
        let w = sqlx::query_as("UPDATE workspaces SET state=$3,revision=revision+1 WHERE id=$1 AND revision=$2 RETURNING *")
            .bind(id).bind(revision).bind(state).fetch_optional(&mut **tx).await?.ok_or_else(|| Error::Conflict("workspace revision changed".into()))?;
        self.event(tx, Some(id), "workspace.updated", json!(w))
            .await?;
        Ok(w)
    }
    pub async fn task(&self, id: Uuid) -> Result<Task> {
        sqlx::query_as("SELECT * FROM tasks WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound("task".into()))
    }
    pub async fn tasks(&self, workspace: Option<Uuid>) -> Result<Vec<Task>> {
        Ok(sqlx::query_as("SELECT * FROM tasks WHERE ($1::uuid IS NULL OR workspace_id=$1) ORDER BY created_at,id").bind(workspace).fetch_all(&self.pool).await?)
    }
    pub async fn create_task(
        &self,
        workspace: Uuid,
        input: &NewTask,
        creator: &str,
        key: Option<&str>,
    ) -> Result<Task> {
        let mut tx = self.pool.begin().await?;
        let task = self
            .create_task_in(&mut tx, workspace, input, creator, key)
            .await?;
        tx.commit().await?;
        Ok(task)
    }
    pub(crate) async fn create_task_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: Uuid,
        input: &NewTask,
        creator: &str,
        key: Option<&str>,
    ) -> Result<Task> {
        nonempty(&input.title, "task title")?;
        nonempty(&input.description, "task description")?;
        if !input.requirements.is_object() {
            return Err(Error::Invalid("requirements must be an object".into()));
        }
        let _: Search = serde_json::from_value(input.requirements.clone())
            .map_err(|e| Error::Invalid(e.to_string()))?;
        for dep in input.dependencies.iter().chain(input.parent_id.iter()) {
            let valid: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=$1 AND workspace_id=$2)",
            )
            .bind(dep)
            .bind(workspace)
            .fetch_one(&mut **tx)
            .await?;
            if !valid {
                return Err(Error::Invalid(
                    "dependencies and parent must belong to the same workspace".into(),
                ));
            }
        }
        let task: Option<Task> = sqlx::query_as("INSERT INTO tasks(id,workspace_id,title,description,requirements,created_by,dependencies,parent_id,creation_key) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(creation_key) DO NOTHING RETURNING *")
            .bind(Uuid::new_v4()).bind(workspace).bind(&input.title).bind(&input.description).bind(&input.requirements).bind(creator).bind(&input.dependencies).bind(input.parent_id).bind(key).fetch_optional(&mut **tx).await?;
        let task = match task {
            Some(t) => {
                self.event(tx, Some(workspace), "task.created", json!(t))
                    .await?;
                t
            }
            None => {
                let t: Task = sqlx::query_as("SELECT * FROM tasks WHERE creation_key=$1")
                    .bind(key)
                    .fetch_one(&mut **tx)
                    .await?;
                if t.workspace_id != workspace
                    || t.title != input.title
                    || t.description != input.description
                    || t.requirements != input.requirements
                    || t.dependencies != input.dependencies
                    || t.parent_id != input.parent_id
                    || t.created_by != creator
                {
                    return Err(Error::Conflict(
                        "idempotency key reused for a different task".into(),
                    ));
                }
                t
            }
        };
        Ok(task)
    }
    pub async fn claim(&self, id: Uuid, revision: i64, owner: &str, agent: &Entry) -> Result<Task> {
        let task = self.task(id).await?;
        self.require_legacy_execution(task.workspace_id).await?;
        let mut tx = self.pool.begin().await?;
        let claimed = self
            .claim_in(&mut tx, &task, revision, owner, agent)
            .await?;
        tx.commit().await?;
        Ok(claimed)
    }
    pub(crate) async fn claim_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        task: &Task,
        revision: i64,
        owner: &str,
        agent: &Entry,
    ) -> Result<Task> {
        let id = task.id;
        let mut requirements: Search = serde_json::from_value(task.requirements.clone())?;
        requirements.kind = Some("agent".into());
        if !requirements.matches(agent) {
            return Err(Error::Invalid(
                "agent does not satisfy task requirements".into(),
            ));
        }
        let claimed: Task = sqlx::query_as("UPDATE tasks SET status='CLAIMED',owner=$3,revision=revision+1 WHERE id=$1 AND revision=$2 AND status='OPEN' AND NOT EXISTS(SELECT 1 FROM tasks d WHERE d.id=ANY(tasks.dependencies) AND d.status <> 'COMPLETED') RETURNING *")
            .bind(id).bind(revision).bind(owner).fetch_optional(&mut **tx).await?.ok_or_else(|| Error::Conflict("task already claimed, revision changed, or dependencies are incomplete".into()))?;
        if owner == qualified_agent(&self.node_id, &agent.id, &agent.version) {
            // Persist the local execution with the claim; no crash can strand a
            // claimed task between the control API and its worker queue.
            sqlx::query("INSERT INTO runs(id,task_id,workspace_id,home_node,agent_id,agent_version) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(home_node,task_id) DO NOTHING")
                .bind(Uuid::new_v4()).bind(id).bind(claimed.workspace_id).bind(&self.node_id)
                .bind(&agent.id).bind(&agent.version).execute(&mut **tx).await?;
        }
        self.event(
            tx,
            Some(claimed.workspace_id),
            "task.claimed",
            json!(claimed),
        )
        .await?;
        Ok(claimed)
    }
    pub async fn transition(
        &self,
        id: Uuid,
        revision: i64,
        owner: &str,
        next: &str,
    ) -> Result<Task> {
        let task = self.task(id).await?;
        let terminate_unclaimed =
            matches!(next, "CANCELLED" | "FAILED") && task.status == "OPEN" && task.owner.is_none();
        if task.owner.as_deref() != Some(owner) && !terminate_unclaimed {
            return Err(Error::Unauthorized);
        }
        let before: TaskStatus = serde_json::from_value(json!(task.status))?;
        let after: TaskStatus = serde_json::from_value(json!(next))
            .map_err(|_| Error::Invalid("invalid task status".into()))?;
        if after == TaskStatus::Completed {
            return Err(Error::Invalid(
                "use completion with an idempotency key".into(),
            ));
        }
        if task.status == next {
            return Ok(task);
        }
        if !before.can_transition(&after) {
            return Err(Error::Conflict(format!(
                "invalid task transition {} -> {next}",
                task.status
            )));
        }
        let mut tx = self.pool.begin().await?;
        let t: Task = sqlx::query_as("UPDATE tasks SET status=$4,owner=CASE WHEN $4='OPEN' THEN NULL ELSE $3 END,revision=revision+1 WHERE id=$1 AND revision=$2 AND (owner=$3 OR (owner IS NULL AND status='OPEN' AND $4 IN ('CANCELLED','FAILED'))) RETURNING *")
            .bind(id).bind(revision).bind(owner).bind(next).fetch_optional(&mut *tx).await?.ok_or_else(|| Error::Conflict("task revision changed".into()))?;
        self.event(&mut tx, Some(t.workspace_id), "task.updated", json!(t))
            .await?;
        tx.commit().await?;
        Ok(t)
    }
    /// Explicit operator abandonment preserves the failed outcome and reason
    /// while allowing the parent to finish using the remaining results.
    pub async fn abandon_task(&self, id: Uuid, revision: i64, reason: &str) -> Result<Task> {
        let mut tx = self.pool.begin().await?;
        let task = self
            .abandon_task_in(&mut tx, id, revision, reason, "human")
            .await?;
        tx.commit().await?;
        Ok(task)
    }
    pub(crate) async fn abandon_task_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        revision: i64,
        reason: &str,
        actor: &str,
    ) -> Result<Task> {
        nonempty(reason, "abandonment reason")?;
        let task: Task = sqlx::query_as("SELECT * FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
        if task.revision != revision
            || !matches!(task.status.as_str(), "FAILED" | "BLOCKED" | "CANCELLED")
        {
            return Err(Error::Conflict(
                "only a failed, blocked or cancelled task at the current revision can be abandoned"
                    .into(),
            ));
        }
        let active_children: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE parent_id=$1 AND status NOT IN ('COMPLETED','ABANDONED'))")
            .bind(id).fetch_one(&mut **tx).await?;
        if active_children {
            return Err(Error::Conflict(
                "resolve or abandon this task's children first".into(),
            ));
        }
        let updated: Task = sqlx::query_as(
            "UPDATE tasks SET status='ABANDONED',revision=revision+1 WHERE id=$1 RETURNING *",
        )
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
        self.event(
            tx,
            Some(task.workspace_id),
            "task.abandoned",
            json!({"task":updated,"previous_status":task.status,"reason":reason,"actor":actor}),
        )
        .await?;
        Ok(updated)
    }
    pub async fn complete(
        &self,
        id: Uuid,
        owner: &str,
        key: &str,
        artifact: &ArtifactInput,
    ) -> Result<Task> {
        nonempty(key, "idempotency key")?;
        artifact.validate()?;
        let mut tx = self.pool.begin().await?;
        let t: Task = sqlx::query_as("SELECT * FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        if t.owner.as_deref() != Some(owner) {
            return Err(Error::Unauthorized);
        }
        let existing: Option<Artifact> =
            sqlx::query_as("SELECT * FROM artifacts WHERE idempotency_key=$1")
                .bind(key)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(a) = existing {
            if a.task_id != id
                || a.created_by != owner
                || a.content != artifact.content
                || a.kind != artifact.kind
                || a.name != artifact.name
                || t.status != "COMPLETED"
            {
                return Err(Error::Conflict(
                    "completion key reused with different input".into(),
                ));
            }
            tx.commit().await?;
            return Ok(t);
        }
        if t.status != "RUNNING" {
            return Err(Error::Conflict("only a running task can complete".into()));
        }
        let a: Artifact = sqlx::query_as("INSERT INTO artifacts(id,workspace_id,task_id,kind,name,content,created_by,idempotency_key) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *")
            .bind(Uuid::new_v4()).bind(t.workspace_id).bind(id).bind(&artifact.kind).bind(&artifact.name).bind(&artifact.content).bind(owner).bind(key).fetch_one(&mut *tx).await?;
        let t: Task = sqlx::query_as("UPDATE tasks SET status='COMPLETED',completion_key=$2,revision=revision+1 WHERE id=$1 RETURNING *").bind(id).bind(key).fetch_one(&mut *tx).await?;
        self.event(
            &mut tx,
            Some(t.workspace_id),
            "task.completed",
            json!({"task":t,"artifact":a}),
        )
        .await?;
        tx.commit().await?;
        Ok(t)
    }
    pub async fn publish_artifact(
        &self,
        task_id: Uuid,
        owner: &str,
        key: &str,
        input: &ArtifactInput,
    ) -> Result<Artifact> {
        input.validate()?;
        let mut tx = self.pool.begin().await?;
        let task: Task = sqlx::query_as("SELECT * FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await?;
        if task.owner.as_deref() != Some(owner) {
            return Err(Error::Unauthorized);
        }
        let a: Option<Artifact> = sqlx::query_as("INSERT INTO artifacts(id,workspace_id,task_id,kind,name,content,created_by,idempotency_key) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(idempotency_key) DO NOTHING RETURNING *")
            .bind(Uuid::new_v4()).bind(task.workspace_id).bind(task_id).bind(&input.kind).bind(&input.name).bind(&input.content).bind(owner).bind(key).fetch_optional(&mut *tx).await?;
        let a = match a {
            Some(a) => {
                self.event(
                    &mut tx,
                    Some(task.workspace_id),
                    "artifact.published",
                    json!(a),
                )
                .await?;
                a
            }
            None => {
                let a: Artifact =
                    sqlx::query_as("SELECT * FROM artifacts WHERE idempotency_key=$1")
                        .bind(key)
                        .fetch_one(&mut *tx)
                        .await?;
                if a.task_id != task_id
                    || a.kind != input.kind
                    || a.name != input.name
                    || a.content != input.content
                {
                    return Err(Error::Conflict("artifact idempotency key reused".into()));
                }
                a
            }
        };
        tx.commit().await?;
        Ok(a)
    }
    pub async fn events(
        &self,
        after: i64,
        workspace: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<Event>> {
        Ok(sqlx::query_as("SELECT sequence,id,node_id,workspace_id,kind,data,created_at FROM events WHERE sequence>$1 AND ($2::uuid IS NULL OR workspace_id=$2) ORDER BY sequence LIMIT $3")
            .bind(after.max(0)).bind(workspace).bind(limit.clamp(1,1000)).fetch_all(&self.pool).await?)
    }
    pub async fn snapshot(&self, id: Uuid) -> Result<WorkspaceSnapshot> {
        Ok(WorkspaceSnapshot {
            workspace: self.workspace(id).await?, tasks: self.tasks(Some(id)).await?,
            artifacts: sqlx::query_as("SELECT * FROM artifacts WHERE workspace_id=$1 ORDER BY created_at").bind(id).fetch_all(&self.pool).await?,
            events: sqlx::query_as("SELECT sequence,id,node_id,workspace_id,kind,data,created_at FROM (SELECT * FROM events WHERE workspace_id=$1 ORDER BY sequence DESC LIMIT 100) e ORDER BY sequence").bind(id).fetch_all(&self.pool).await?,
            messages: sqlx::query_as("SELECT * FROM (SELECT * FROM messages WHERE workspace_id=$1 ORDER BY created_at DESC LIMIT 100) m ORDER BY created_at").bind(id).fetch_all(&self.pool).await?,
        })
    }
    pub async fn message(
        &self,
        workspace: Uuid,
        sender: &str,
        content: &str,
        key: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.message_in(&mut tx, workspace, sender, content, key)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn message_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: Uuid,
        sender: &str,
        content: &str,
        key: Option<&str>,
    ) -> Result<()> {
        nonempty(content, "message")?;
        let inserted = sqlx::query("INSERT INTO messages(id,workspace_id,sender,content,idempotency_key) VALUES($1,$2,$3,$4,$5) ON CONFLICT(idempotency_key) DO NOTHING")
            .bind(Uuid::new_v4()).bind(workspace).bind(sender).bind(content).bind(key).execute(&mut **tx).await?.rows_affected();
        if inserted > 0 {
            self.event(
                tx,
                Some(workspace),
                "message.created",
                json!({"sender":sender,"content":content}),
            )
            .await?;
        } else {
            let matches: bool = sqlx::query_scalar("SELECT workspace_id=$2 AND sender=$3 AND content=$4 FROM messages WHERE idempotency_key=$1")
                .bind(key).bind(workspace).bind(sender).bind(content).fetch_one(&mut **tx).await?;
            if !matches {
                return Err(Error::Conflict("message idempotency key reused".into()));
            }
        }
        Ok(())
    }
}

impl Store {
    pub async fn accept_run(
        &self,
        task: &Task,
        home_node: &str,
        agent_id: &str,
        agent_version: &str,
    ) -> Result<Run> {
        self.require_legacy_execution(task.workspace_id).await?;
        let mut tx = self.pool.begin().await?;
        let row: Option<Run> = sqlx::query_as("INSERT INTO runs(id,task_id,workspace_id,home_node,agent_id,agent_version) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(home_node,task_id) DO NOTHING RETURNING *")
            .bind(Uuid::new_v4()).bind(task.id).bind(task.workspace_id).bind(home_node).bind(agent_id).bind(agent_version).fetch_optional(&mut *tx).await?;
        let run = match row {
            Some(r) => {
                self.event(
                    &mut tx,
                    (home_node == self.node_id).then_some(task.workspace_id),
                    "run.created",
                    json!(r),
                )
                .await?;
                r
            }
            None => {
                let r: Run = sqlx::query_as("SELECT * FROM runs WHERE home_node=$1 AND task_id=$2")
                    .bind(home_node)
                    .bind(task.id)
                    .fetch_one(&mut *tx)
                    .await?;
                if r.agent_id != agent_id
                    || r.agent_version != agent_version
                    || r.workspace_id != task.workspace_id
                {
                    return Err(Error::Conflict(
                        "task already has a different executor".into(),
                    ));
                }
                r
            }
        };
        tx.commit().await?;
        Ok(run)
    }
    pub async fn run(&self, id: Uuid) -> Result<Run> {
        sqlx::query_as("SELECT * FROM runs WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound("run".into()))
    }
    pub async fn runs(&self) -> Result<Vec<Run>> {
        Ok(
            sqlx::query_as("SELECT * FROM runs ORDER BY updated_at DESC LIMIT 500")
                .fetch_all(&self.pool)
                .await?,
        )
    }
    pub async fn lease_run(&self, worker: Uuid, seconds: i32) -> Result<Option<Run>> {
        // SKIP LOCKED permits independent workers; the token fences stale writers.
        Ok(sqlx::query_as("UPDATE runs SET pending=CASE WHEN lease_owner IS NOT NULL THEN pending || '{\"lease_recovered\":true}'::jsonb ELSE pending END,lease_owner=$1,lease_until=now()+make_interval(secs=>$2),revision=revision+1 WHERE id=(SELECT id FROM runs WHERE phase NOT IN ('COMPLETED','FAILED','CANCELLED') AND control <> 'PAUSED' AND (lease_until IS NULL OR lease_until<now()) AND (NOT (pending ? 'retry_at') OR (pending->>'retry_at')::timestamptz<now()) AND (phase <> 'WAITING' OR control='CANCELLED' OR (pending->>'wake_at')::timestamptz<now() OR EXISTS(SELECT 1 FROM human_requests h WHERE h.id::text=runs.pending->>'human_request_id' AND h.response IS NOT NULL)) ORDER BY updated_at FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING *")
            .bind(worker).bind(seconds as f64).fetch_optional(&self.pool).await?)
    }
    pub async fn renew_lease(&self, id: Uuid, worker: Uuid, seconds: i32) -> Result<bool> {
        Ok(sqlx::query("UPDATE runs SET lease_until=now()+make_interval(secs=>$3) WHERE id=$1 AND lease_owner=$2 AND lease_until>now()")
            .bind(id).bind(worker).bind(seconds as f64).execute(&self.pool).await?.rows_affected() == 1)
    }
    pub async fn save_run(&self, run: &Run, worker: Uuid, kind: &str) -> Result<Run> {
        let mut pending = run.pending.clone();
        let retrying = matches!(kind, "run.retrying" | "run.failure_pending");
        if !retrying && let Some(object) = pending.as_object_mut() {
            object.remove("retry_count");
            object.remove("retry_at");
        }
        let error = if retrying || kind == "run.failed" {
            run.error.as_deref()
        } else {
            None
        };
        let mut tx = self.pool.begin().await?;
        let saved: Run = sqlx::query_as("UPDATE runs SET phase=$3,context=$4,pending=$5,step=$6,error=$7,revision=revision+1,updated_at=now(),lease_owner=NULL,lease_until=NULL WHERE id=$1 AND lease_owner=$2 AND lease_until>now() RETURNING *")
            .bind(run.id).bind(worker).bind(&run.phase).bind(&run.context).bind(&pending).bind(run.step).bind(error).fetch_optional(&mut *tx).await?.ok_or_else(|| Error::Conflict("worker lease lost".into()))?;
        self.event(&mut tx, (run.home_node == self.node_id).then_some(run.workspace_id), kind,
            json!({"run_id":saved.id,"task_id":saved.task_id,"workspace_id":saved.workspace_id,"agent_id":saved.agent_id,"phase":saved.phase,"step":saved.step,"error":saved.error,"context_usage":saved.context.get("usage")})).await?;
        tx.commit().await?;
        Ok(saved)
    }
    pub async fn release_lease(&self, id: Uuid, worker: Uuid) -> Result<()> {
        sqlx::query("UPDATE runs SET lease_owner=NULL,lease_until=NULL,updated_at=now() WHERE id=$1 AND lease_owner=$2").bind(id).bind(worker).execute(&self.pool).await?;
        Ok(())
    }
    pub(crate) async fn pause_for_authorization(&self, run: &Run, worker: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let changed=sqlx::query("UPDATE runs SET control=CASE WHEN control='CANCELLED' THEN control ELSE 'PAUSED' END,error='execution authority denied',revision=revision+1,updated_at=now(),lease_owner=NULL,lease_until=NULL WHERE id=$1 AND lease_owner=$2 AND lease_until>now()")
            .bind(run.id).bind(worker).execute(&mut *tx).await?.rows_affected();
        if changed == 0 {
            return Err(Error::Conflict("worker lease lost".into()));
        }
        self.event(
            &mut tx,
            Some(run.workspace_id),
            "run.authorization_blocked",
            json!({"run_id":run.id,"task_id":run.task_id}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn cancel_execution(&self, run: &Run, worker: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let valid:Option<Uuid>=sqlx::query_scalar("SELECT id FROM runs WHERE id=$1 AND lease_owner=$2 AND lease_until>now() AND control='CANCELLED' FOR UPDATE")
            .bind(run.id).bind(worker).fetch_optional(&mut *tx).await?;
        if valid.is_none() {
            return Err(Error::Conflict("worker lease lost".into()));
        }
        let task: Task = sqlx::query_as("SELECT * FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(run.task_id)
            .fetch_one(&mut *tx)
            .await?;
        let phase = if task.status == "COMPLETED" {
            "COMPLETED"
        } else {
            "CANCELLED"
        };
        if !matches!(
            task.status.as_str(),
            "COMPLETED" | "CANCELLED" | "ABANDONED"
        ) {
            let task: Task = sqlx::query_as(
                "UPDATE tasks SET status='CANCELLED',revision=revision+1 WHERE id=$1 RETURNING *",
            )
            .bind(run.task_id)
            .fetch_one(&mut *tx)
            .await?;
            self.event(&mut tx, Some(run.workspace_id), "task.updated", json!(task))
                .await?;
        }
        sqlx::query("UPDATE runs SET phase=$3,pending='{}',error=NULL,revision=revision+1,updated_at=now(),lease_owner=NULL,lease_until=NULL WHERE id=$1 AND lease_owner=$2")
            .bind(run.id).bind(worker).bind(phase).execute(&mut *tx).await?;
        self.event(
            &mut tx,
            Some(run.workspace_id),
            if phase == "CANCELLED" {
                "run.cancelled"
            } else {
                "run.reconciled"
            },
            json!({"run_id":run.id,"task_id":run.task_id}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn control(&self, id: Uuid, action: &str) -> Result<Run> {
        let mut tx = self.pool.begin().await?;
        let run = self.control_in(&mut tx, id, action).await?;
        tx.commit().await?;
        Ok(run)
    }
    pub(crate) async fn control_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        action: &str,
    ) -> Result<Run> {
        let control = match action {
            "pause" => "PAUSED",
            "resume" => "ACTIVE",
            "cancel" => "CANCELLED",
            _ => {
                return Err(Error::Invalid(
                    "control must be pause, resume or cancel".into(),
                ));
            }
        };
        let r: Run = sqlx::query_as("UPDATE runs SET control=$2,revision=revision+1,updated_at=now() WHERE id=$1 AND phase NOT IN ('COMPLETED','FAILED','CANCELLED') AND control <> 'CANCELLED' RETURNING *")
            .bind(id).bind(control).fetch_optional(&mut **tx).await?.ok_or_else(|| Error::Conflict("run is terminal or cancellation is already requested".into()))?;
        self.event(
            tx,
            (r.home_node == self.node_id).then_some(r.workspace_id),
            "run.control",
            json!({"run_id":id,"action":action}),
        )
        .await?;
        Ok(r)
    }
    pub async fn human_request(
        &self,
        run: &Run,
        kind: &str,
        prompt: &str,
        key: &str,
    ) -> Result<HumanRequest> {
        if !matches!(
            kind,
            "QUESTION" | "APPROVAL_REQUIRED" | "CONFIRMATION" | "INFORMATION_REQUEST"
        ) {
            return Err(Error::Invalid("unknown human request kind".into()));
        }
        nonempty(prompt, "human request prompt")?;
        let mut tx = self.pool.begin().await?;
        let h: Option<HumanRequest> = sqlx::query_as("INSERT INTO human_requests(id,workspace_id,run_id,kind,prompt,request_key) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(request_key) DO NOTHING RETURNING *")
            .bind(Uuid::new_v4()).bind(run.workspace_id).bind(run.id).bind(kind).bind(prompt).bind(key).fetch_optional(&mut *tx).await?;
        let h = match h {
            Some(h) => {
                self.event(
                    &mut tx,
                    (run.home_node == self.node_id).then_some(run.workspace_id),
                    "human.requested",
                    json!(h),
                )
                .await?;
                h
            }
            None => {
                sqlx::query_as("SELECT * FROM human_requests WHERE request_key=$1")
                    .bind(key)
                    .fetch_one(&mut *tx)
                    .await?
            }
        };
        if h.run_id != run.id || h.kind != kind || h.prompt != prompt {
            return Err(Error::Conflict(
                "human request key reused with different input".into(),
            ));
        }
        tx.commit().await?;
        Ok(h)
    }
    pub async fn answer(&self, id: Uuid, response: Value) -> Result<HumanRequest> {
        let mut tx = self.pool.begin().await?;
        let request = self.answer_in(&mut tx, id, response, "human").await?;
        tx.commit().await?;
        Ok(request)
    }
    pub(crate) async fn answer_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        response: Value,
        actor: &str,
    ) -> Result<HumanRequest> {
        if response.is_null() {
            return Err(Error::Invalid("a human response cannot be null".into()));
        }
        let old: HumanRequest =
            sqlx::query_as("SELECT * FROM human_requests WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or_else(|| Error::NotFound("human request".into()))?;
        if let Some(existing) = &old.response {
            if existing != &response {
                return Err(Error::Conflict("human request already answered".into()));
            }
            return Ok(old);
        }
        let run: Run = sqlx::query_as("SELECT * FROM runs WHERE id=$1")
            .bind(old.run_id)
            .fetch_one(&mut **tx)
            .await?;
        if run
            .pending
            .get("uncertain_key")
            .and_then(Value::as_str)
            .is_some()
            && response.get("result").is_none()
        {
            return Err(Error::Invalid(
                "tool reconciliation requires a JSON object containing result".into(),
            ));
        }
        let h: HumanRequest = sqlx::query_as(
            "UPDATE human_requests SET response=$2,answered_by=$3 WHERE id=$1 RETURNING *",
        )
        .bind(id)
        .bind(response)
        .bind(actor)
        .fetch_one(&mut **tx)
        .await?;
        self.event(
            tx,
            (run.home_node == self.node_id).then_some(run.workspace_id),
            "human.answered",
            json!(h),
        )
        .await?;
        Ok(h)
    }
    pub async fn invocation_start(
        &self,
        run: &Run,
        worker: Uuid,
        key: &str,
        tool: &str,
        input: &Value,
        replay_safe: bool,
    ) -> Result<Invocation> {
        let mut tx = self.pool.begin().await?;
        let valid: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM runs WHERE id=$1 AND lease_owner=$2 AND lease_until>now() FOR UPDATE",
        )
        .bind(run.id)
        .bind(worker)
        .fetch_optional(&mut *tx)
        .await?;
        if valid.is_none() {
            return Err(Error::Conflict(
                "worker lease lost before tool invocation".into(),
            ));
        }
        let created = sqlx::query("INSERT INTO invocations(idempotency_key,run_id,tool,input,status,replay_safe) VALUES($1,$2,$3,$4,'STARTED',$5) ON CONFLICT DO NOTHING")
            .bind(key).bind(run.id).bind(tool).bind(input).bind(replay_safe).execute(&mut *tx).await?.rows_affected() == 1;
        let mut invocation: Invocation =
            sqlx::query_as("SELECT * FROM invocations WHERE idempotency_key=$1")
                .bind(key)
                .fetch_one(&mut *tx)
                .await?;
        if invocation.input != *input || invocation.tool != tool || invocation.run_id != run.id {
            return Err(Error::Conflict(
                "tool idempotency key reused with different input".into(),
            ));
        }
        if !created && invocation.status != "COMPLETED" && !invocation.replay_safe {
            sqlx::query("UPDATE invocations SET status='UNCERTAIN' WHERE idempotency_key=$1")
                .bind(key)
                .execute(&mut *tx)
                .await?;
            invocation.status = "UNCERTAIN".into();
        }
        if created {
            self.event(
                &mut tx,
                (run.home_node == self.node_id).then_some(run.workspace_id),
                "tool.started",
                json!({"run_id":run.id,"tool":tool,"idempotency_key":key,"input":input}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(invocation)
    }
    pub async fn invocation_finish(
        &self,
        run: &Run,
        worker: Uuid,
        key: &str,
        result: &Value,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let valid: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM runs WHERE id=$1 AND lease_owner=$2 AND lease_until>now() FOR UPDATE",
        )
        .bind(run.id)
        .bind(worker)
        .fetch_optional(&mut *tx)
        .await?;
        if valid.is_none() {
            return Err(Error::Conflict(
                "worker lease lost while recording tool result".into(),
            ));
        }
        let changed = sqlx::query("UPDATE invocations SET status='COMPLETED',result=$3 WHERE idempotency_key=$1 AND run_id=$2 AND status <> 'COMPLETED'")
            .bind(key).bind(run.id).bind(result).execute(&mut *tx).await?.rows_affected();
        if changed > 0 {
            self.event(
                &mut tx,
                (run.home_node == self.node_id).then_some(run.workspace_id),
                "tool.completed",
                json!({"run_id":run.id,"idempotency_key":key,"result":result}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn remember(&self, run: &Run, data: &Value) -> Result<()> {
        sqlx::query("INSERT INTO memory(agent_id,agent_version,workspace_id,data) VALUES($1,$2,$3,$4) ON CONFLICT(agent_id,agent_version,workspace_id) DO UPDATE SET data=EXCLUDED.data")
            .bind(&run.agent_id).bind(&run.agent_version).bind(run.workspace_id).bind(data).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn memory(&self, run: &Run) -> Result<Value> {
        Ok(sqlx::query_scalar(
            "SELECT data FROM memory WHERE agent_id=$1 AND agent_version=$2 AND workspace_id=$3",
        )
        .bind(&run.agent_id)
        .bind(&run.agent_version)
        .bind(run.workspace_id)
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or_else(empty_object))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Invocation {
    pub idempotency_key: String,
    pub run_id: Uuid,
    pub tool: String,
    pub input: Value,
    pub status: String,
    pub result: Option<Value>,
    pub replay_safe: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
