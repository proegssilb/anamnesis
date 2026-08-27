//! In-memory fakes for every `anamnesis-app` port against the real domain
//! model (`docs/DOMAIN.md`). Not production code — nothing here is compiled
//! into the `anamnesis-app` library; it exists purely so the use-case tests
//! and the (extended) cucumber suite can exercise real orchestration logic
//! without a database.
//!
//! One struct, [`Fakes`], implements every port at once, backed by shared
//! `Mutex`-guarded state. This is deliberate, not laziness: several use
//! cases (`raise_task`/`drop_task` against `BoardQuery`, `request_suggestion`
//! against both `BoardQuery` and `TaskRepository`) need their ports to agree
//! about the same underlying tasks, so one shared store is what makes a
//! fake actually behave like a smaller, honest version of a real backend
//! rather than two disconnected mocks that happen to compile.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use anamnesis_app::{
    Attachment, AttachmentId, AttachmentRepository, AreaRepository, BlobStore, BoardColumn,
    BoardQuery, Comment, CommentId, CommentRepository, MembershipQuery, ProjectAggregate,
    ProjectRepository, RelationshipRepository, RepoError, SearchHit, SearchIndex, SearchQuery,
    TangleRepository, TaskAggregate, TaskRepository, TaskUpdateError,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{
    Area, AreaId, BlockingGraph, BoardState, Column, ColumnId, FieldDefinition, FieldValue,
    KindId, Project, ProjectId, ProjectStatus, Relationship, RelationshipId, RelationshipKind,
    Tangle, Task, TaskId, TaskSummary, Timestamp, UserId,
};

/// Shared in-memory backing store implementing every real-domain-model port.
#[derive(Default)]
pub struct Fakes {
    areas: Mutex<HashMap<AreaId, Area>>,
    projects: Mutex<HashMap<ProjectId, ProjectAggregate>>,
    tasks: Mutex<HashMap<TaskId, TaskAggregate>>,
    relationships: Mutex<HashMap<RelationshipId, Relationship>>,
    tangles: Mutex<HashMap<anamnesis_core::TangleId, Tangle>>,
    comments: Mutex<HashMap<CommentId, Comment>>,
    attachments: Mutex<HashMap<AttachmentId, Attachment>>,
    columns: Mutex<Vec<Column>>,
    blobs: Mutex<HashMap<String, (Vec<u8>, String)>>,
    system_admins: Mutex<HashMap<UserId, bool>>,
    project_roles: Mutex<HashMap<(UserId, ProjectId), Role>>,
    /// Every `(kind, id, title)` ever indexed, minus anything removed —
    /// good enough for `support_doubles`-style assertions on `SearchIndex`.
    search_entries: Mutex<Vec<(&'static str, String, String)>>,
}

impl Fakes {
    pub fn new() -> Self {
        Self::default()
    }

    // --- test setup helpers (not part of any port) ---

    pub fn seed_area(&self, area: Area) {
        self.areas.lock().unwrap().insert(area.id, area);
    }

    pub fn seed_project(&self, project: Project) {
        self.projects.lock().unwrap().insert(
            project.id,
            ProjectAggregate {
                project,
                field_definitions: Vec::new(),
                relationship_kinds: Vec::new(),
            },
        );
    }

    pub fn seed_task(&self, task: Task) {
        self.tasks.lock().unwrap().insert(
            task.id,
            TaskAggregate {
                task,
                field_values: Vec::new(),
            },
        );
    }

    pub fn seed_column(&self, column: Column) {
        self.columns.lock().unwrap().push(column);
    }

    pub fn seed_relationship(&self, relationship: Relationship) {
        self.relationships
            .lock()
            .unwrap()
            .insert(relationship.id, relationship);
    }

    pub fn seed_tangle(&self, tangle: Tangle) {
        self.tangles.lock().unwrap().insert(tangle.id, tangle);
    }

    pub fn seed_comment(&self, comment: Comment) {
        self.comments.lock().unwrap().insert(comment.id, comment);
    }

    /// Grants `user` System Admin.
    pub fn make_system_admin(&self, user: &UserId) {
        self.system_admins
            .lock()
            .unwrap()
            .insert(user.clone(), true);
    }

    /// Grants `user` `role` on `project`.
    pub fn set_project_role(&self, user: &UserId, project: ProjectId, role: Role) {
        self.project_roles
            .lock()
            .unwrap()
            .insert((user.clone(), project), role);
    }

    /// Reads a task straight out of the store, bypassing any use case —
    /// what `Then` assertions check against.
    pub fn task(&self, id: TaskId) -> Task {
        self.tasks
            .lock()
            .unwrap()
            .get(&id)
            .expect("task must exist in the fake store")
            .task
            .clone()
    }

    pub fn project(&self, id: ProjectId) -> Project {
        self.projects
            .lock()
            .unwrap()
            .get(&id)
            .expect("project must exist in the fake store")
            .project
            .clone()
    }
}

#[async_trait]
impl AreaRepository for Fakes {
    async fn load(&self, id: AreaId) -> Result<Option<Area>, RepoError> {
        Ok(self.areas.lock().unwrap().get(&id).cloned())
    }

    async fn list(&self) -> Result<Vec<Area>, RepoError> {
        let mut areas: Vec<Area> = self.areas.lock().unwrap().values().cloned().collect();
        areas.sort_by_key(|a| a.position);
        Ok(areas)
    }

    async fn insert(&self, area: &Area) -> Result<(), RepoError> {
        self.areas.lock().unwrap().insert(area.id, area.clone());
        Ok(())
    }

    async fn update(&self, area: &Area) -> Result<(), RepoError> {
        self.areas.lock().unwrap().insert(area.id, area.clone());
        Ok(())
    }
}

#[async_trait]
impl ProjectRepository for Fakes {
    async fn load(&self, id: ProjectId) -> Result<Option<ProjectAggregate>, RepoError> {
        Ok(self.projects.lock().unwrap().get(&id).cloned())
    }

    async fn list_by_area(&self, area_id: AreaId) -> Result<Vec<Project>, RepoError> {
        Ok(self
            .projects
            .lock()
            .unwrap()
            .values()
            .map(|agg| agg.project.clone())
            .filter(|p| p.area_id == area_id)
            .collect())
    }

    async fn count_active(&self, excluding: Option<ProjectId>) -> Result<u32, RepoError> {
        Ok(self
            .projects
            .lock()
            .unwrap()
            .values()
            .filter(|agg| agg.project.status == ProjectStatus::Active)
            .filter(|agg| Some(agg.project.id) != excluding)
            .count() as u32)
    }

    async fn insert(&self, project: &Project) -> Result<(), RepoError> {
        self.projects.lock().unwrap().insert(
            project.id,
            ProjectAggregate {
                project: project.clone(),
                field_definitions: Vec::new(),
                relationship_kinds: Vec::new(),
            },
        );
        Ok(())
    }

    async fn update(&self, project: &Project) -> Result<(), RepoError> {
        let mut projects = self.projects.lock().unwrap();
        let agg = projects
            .get_mut(&project.id)
            .ok_or_else(|| RepoError::new("no such project"))?;
        agg.project = project.clone();
        Ok(())
    }

    async fn insert_field_definition(
        &self,
        definition: &FieldDefinition,
    ) -> Result<(), RepoError> {
        let mut projects = self.projects.lock().unwrap();
        let agg = projects
            .get_mut(&definition.project_id)
            .ok_or_else(|| RepoError::new("no such project"))?;
        agg.field_definitions.push(definition.clone());
        Ok(())
    }

    async fn update_field_definition(
        &self,
        definition: &FieldDefinition,
    ) -> Result<(), RepoError> {
        let mut projects = self.projects.lock().unwrap();
        let agg = projects
            .get_mut(&definition.project_id)
            .ok_or_else(|| RepoError::new("no such project"))?;
        if let Some(existing) = agg
            .field_definitions
            .iter_mut()
            .find(|d| d.id == definition.id)
        {
            *existing = definition.clone();
        }
        Ok(())
    }

    async fn insert_relationship_kind(&self, kind: &RelationshipKind) -> Result<(), RepoError> {
        let project_id = kind
            .project_id
            .ok_or_else(|| RepoError::new("cannot store a builtin kind"))?;
        let mut projects = self.projects.lock().unwrap();
        let agg = projects
            .get_mut(&project_id)
            .ok_or_else(|| RepoError::new("no such project"))?;
        agg.relationship_kinds.push(kind.clone());
        Ok(())
    }

    async fn load_relationship_kind(
        &self,
        id: KindId,
    ) -> Result<Option<RelationshipKind>, RepoError> {
        Ok(self
            .projects
            .lock()
            .unwrap()
            .values()
            .flat_map(|agg| agg.relationship_kinds.iter())
            .find(|k| k.id == id)
            .cloned())
    }
}

#[async_trait]
impl TaskRepository for Fakes {
    async fn load(&self, id: TaskId) -> Result<Option<TaskAggregate>, RepoError> {
        Ok(self.tasks.lock().unwrap().get(&id).cloned())
    }

    async fn list_children(&self, parent_id: TaskId) -> Result<Vec<Task>, RepoError> {
        Ok(self
            .tasks
            .lock()
            .unwrap()
            .values()
            .map(|agg| agg.task.clone())
            .filter(|t| t.parent_task_id == Some(parent_id))
            .collect())
    }

    async fn insert(&self, task: &Task) -> Result<(), RepoError> {
        self.tasks.lock().unwrap().insert(
            task.id,
            TaskAggregate {
                task: task.clone(),
                field_values: Vec::new(),
            },
        );
        Ok(())
    }

    async fn update(
        &self,
        task: &Task,
        expected_last_touched_at: Timestamp,
    ) -> Result<(), TaskUpdateError> {
        let mut tasks = self.tasks.lock().unwrap();
        let agg = tasks
            .get_mut(&task.id)
            .ok_or_else(|| TaskUpdateError::Repo(RepoError::new("no such task")))?;
        if agg.task.last_touched_at != expected_last_touched_at {
            return Err(TaskUpdateError::Conflict);
        }
        agg.task = task.clone();
        Ok(())
    }

    async fn set_field_value(&self, value: &FieldValue) -> Result<(), RepoError> {
        let mut tasks = self.tasks.lock().unwrap();
        let agg = tasks
            .get_mut(&value.task_id)
            .ok_or_else(|| RepoError::new("no such task"))?;
        if let Some(existing) = agg
            .field_values
            .iter_mut()
            .find(|v| v.field_id == value.field_id)
        {
            *existing = value.clone();
        } else {
            agg.field_values.push(value.clone());
        }
        Ok(())
    }
}

#[async_trait]
impl RelationshipRepository for Fakes {
    async fn load(&self, id: RelationshipId) -> Result<Option<Relationship>, RepoError> {
        Ok(self.relationships.lock().unwrap().get(&id).cloned())
    }

    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<Relationship>, RepoError> {
        Ok(self
            .relationships
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.from_task_id == task_id || r.to_task_id == task_id)
            .cloned()
            .collect())
    }

    async fn list_blocking(&self) -> Result<Vec<Relationship>, RepoError> {
        Ok(self
            .relationships
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.kind_id == KindId::BUILTIN_BLOCKS)
            .cloned()
            .collect())
    }

    async fn insert(&self, relationship: &Relationship) -> Result<(), RepoError> {
        self.relationships
            .lock()
            .unwrap()
            .insert(relationship.id, relationship.clone());
        Ok(())
    }

    async fn delete(&self, id: RelationshipId) -> Result<(), RepoError> {
        self.relationships.lock().unwrap().remove(&id);
        Ok(())
    }
}

#[async_trait]
impl TangleRepository for Fakes {
    async fn list_active(&self) -> Result<Vec<Tangle>, RepoError> {
        Ok(self
            .tangles
            .lock()
            .unwrap()
            .values()
            .filter(|t| t.is_active())
            .cloned()
            .collect())
    }

    async fn insert(&self, tangle: &Tangle) -> Result<(), RepoError> {
        self.tangles.lock().unwrap().insert(tangle.id, tangle.clone());
        Ok(())
    }

    async fn update(&self, tangle: &Tangle) -> Result<(), RepoError> {
        self.tangles.lock().unwrap().insert(tangle.id, tangle.clone());
        Ok(())
    }
}

#[async_trait]
impl CommentRepository for Fakes {
    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<Comment>, RepoError> {
        Ok(self
            .comments
            .lock()
            .unwrap()
            .values()
            .filter(|c| c.task_id == task_id)
            .cloned()
            .collect())
    }

    async fn load(&self, id: CommentId) -> Result<Option<Comment>, RepoError> {
        Ok(self.comments.lock().unwrap().get(&id).cloned())
    }

    async fn insert(&self, comment: &Comment) -> Result<(), RepoError> {
        self.comments
            .lock()
            .unwrap()
            .insert(comment.id, comment.clone());
        Ok(())
    }

    async fn update(&self, comment: &Comment) -> Result<(), RepoError> {
        self.comments
            .lock()
            .unwrap()
            .insert(comment.id, comment.clone());
        Ok(())
    }

    async fn delete(&self, id: CommentId) -> Result<(), RepoError> {
        self.comments.lock().unwrap().remove(&id);
        Ok(())
    }
}

#[async_trait]
impl AttachmentRepository for Fakes {
    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<Attachment>, RepoError> {
        Ok(self
            .attachments
            .lock()
            .unwrap()
            .values()
            .filter(|a| a.task_id == task_id)
            .cloned()
            .collect())
    }

    async fn load(&self, id: AttachmentId) -> Result<Option<Attachment>, RepoError> {
        Ok(self.attachments.lock().unwrap().get(&id).cloned())
    }

    async fn insert(&self, attachment: &Attachment) -> Result<(), RepoError> {
        self.attachments
            .lock()
            .unwrap()
            .insert(attachment.id, attachment.clone());
        Ok(())
    }

    async fn delete(&self, id: AttachmentId) -> Result<(), RepoError> {
        self.attachments.lock().unwrap().remove(&id);
        Ok(())
    }
}

#[async_trait]
impl BlobStore for Fakes {
    async fn put(&self, key: &str, bytes: Vec<u8>, mime: &str) -> Result<(), RepoError> {
        self.blobs
            .lock()
            .unwrap()
            .insert(key.to_string(), (bytes, mime.to_string()));
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, RepoError> {
        Ok(self
            .blobs
            .lock()
            .unwrap()
            .get(key)
            .map(|(bytes, _)| bytes.clone()))
    }

    async fn delete(&self, key: &str) -> Result<(), RepoError> {
        self.blobs.lock().unwrap().remove(key);
        Ok(())
    }
}

#[async_trait]
impl SearchIndex for Fakes {
    async fn index_area(&self, id: AreaId, title: &str) -> Result<(), RepoError> {
        self.search_entries
            .lock()
            .unwrap()
            .push(("area", id.to_string(), title.to_string()));
        Ok(())
    }

    async fn index_project(&self, id: ProjectId, title: &str) -> Result<(), RepoError> {
        self.search_entries
            .lock()
            .unwrap()
            .push(("project", id.to_string(), title.to_string()));
        Ok(())
    }

    async fn index_task(&self, id: TaskId, title: &str) -> Result<(), RepoError> {
        self.search_entries
            .lock()
            .unwrap()
            .push(("task", id.to_string(), title.to_string()));
        Ok(())
    }

    async fn remove_area(&self, id: AreaId) -> Result<(), RepoError> {
        self.search_entries
            .lock()
            .unwrap()
            .retain(|(kind, entry_id, _)| !(*kind == "area" && *entry_id == id.to_string()));
        Ok(())
    }

    async fn remove_project(&self, id: ProjectId) -> Result<(), RepoError> {
        self.search_entries
            .lock()
            .unwrap()
            .retain(|(kind, entry_id, _)| !(*kind == "project" && *entry_id == id.to_string()));
        Ok(())
    }

    async fn remove_task(&self, id: TaskId) -> Result<(), RepoError> {
        self.search_entries
            .lock()
            .unwrap()
            .retain(|(kind, entry_id, _)| !(*kind == "task" && *entry_id == id.to_string()));
        Ok(())
    }
}

#[async_trait]
impl SearchQuery for Fakes {
    async fn search(&self, text: &str) -> Result<Vec<SearchHit>, RepoError> {
        let needle = text.to_lowercase();
        Ok(self
            .search_entries
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, _, title)| title.to_lowercase().contains(&needle))
            .map(|(kind, id, title)| match *kind {
                "area" => SearchHit::Area {
                    id: AreaId::new(uuid::Uuid::parse_str(id).unwrap()),
                    title: title.clone(),
                },
                "project" => SearchHit::Project {
                    id: ProjectId::new(uuid::Uuid::parse_str(id).unwrap()),
                    title: title.clone(),
                },
                _ => SearchHit::Task {
                    id: TaskId::new(uuid::Uuid::parse_str(id).unwrap()),
                    title: title.clone(),
                },
            })
            .collect())
    }
}

#[async_trait]
impl MembershipQuery for Fakes {
    async fn is_system_admin(&self, user: &UserId) -> Result<bool, RepoError> {
        Ok(self
            .system_admins
            .lock()
            .unwrap()
            .get(user)
            .copied()
            .unwrap_or(false))
    }

    async fn project_role(
        &self,
        user: &UserId,
        project: ProjectId,
    ) -> Result<Option<Role>, RepoError> {
        Ok(self
            .project_roles
            .lock()
            .unwrap()
            .get(&(user.clone(), project))
            .copied())
    }
}

#[async_trait]
impl BoardQuery for Fakes {
    async fn columns_with_tasks(&self) -> Result<Vec<BoardColumn>, RepoError> {
        let columns = self.columns.lock().unwrap().clone();
        let tasks = self.tasks.lock().unwrap();
        let mut result: Vec<BoardColumn> = columns
            .into_iter()
            .map(|column| {
                let mut on_column: Vec<Task> = tasks
                    .values()
                    .map(|agg| agg.task.clone())
                    .filter(|t| t.archived_at.is_none())
                    .filter(|t| {
                        matches!(
                            t.placement,
                            anamnesis_core::Placement::OnBoard { column: c, .. } if c == column.id
                        )
                    })
                    .collect();
                on_column.sort_by_key(|t| match t.placement {
                    anamnesis_core::Placement::OnBoard { position, .. } => position,
                    anamnesis_core::Placement::Below => u32::MAX,
                });
                BoardColumn {
                    column,
                    tasks: on_column,
                }
            })
            .collect();
        result.sort_by_key(|bc| bc.column.position);
        Ok(result)
    }

    async fn count_on_column(&self, column: ColumnId) -> Result<u32, RepoError> {
        Ok(self
            .tasks
            .lock()
            .unwrap()
            .values()
            .filter(|agg| agg.task.archived_at.is_none())
            .filter(|agg| {
                matches!(
                    agg.task.placement,
                    anamnesis_core::Placement::OnBoard { column: c, .. } if c == column
                )
            })
            .count() as u32)
    }

    async fn board_state(&self, column: ColumnId) -> Result<BoardState, RepoError> {
        let wip_limit = self
            .columns
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.id == column)
            .and_then(|c| c.wip_limit);
        let current_count = self.count_on_column(column).await?;
        Ok(BoardState {
            wip_limit,
            current_count,
        })
    }

    async fn suggestion_candidates(&self) -> Result<Vec<TaskSummary>, RepoError> {
        let tasks = self.tasks.lock().unwrap();
        let projects = self.projects.lock().unwrap();
        Ok(tasks
            .values()
            .map(|agg| {
                let status = projects
                    .get(&agg.task.project_id)
                    .map(|p| p.project.status)
                    .unwrap_or(ProjectStatus::Pending);
                TaskSummary::from_task(&agg.task, status)
            })
            .collect())
    }

    async fn blocking_graph(&self) -> Result<BlockingGraph, RepoError> {
        let relationships = self.relationships.lock().unwrap();
        let edges: Vec<(TaskId, TaskId)> = relationships
            .values()
            .filter(|r| r.kind_id == KindId::BUILTIN_BLOCKS)
            .map(|r| (r.from_task_id, r.to_task_id))
            .collect();
        drop(relationships);

        let columns = self.columns.lock().unwrap();
        let done_columns: std::collections::HashSet<ColumnId> = columns
            .iter()
            .filter(|c| c.is_done)
            .map(|c| c.id)
            .collect();
        drop(columns);

        let tasks = self.tasks.lock().unwrap();
        let done_task_ids: std::collections::BTreeSet<TaskId> = tasks
            .values()
            .filter(|agg| {
                matches!(
                    agg.task.placement,
                    anamnesis_core::Placement::OnBoard { column, .. } if done_columns.contains(&column)
                )
            })
            .map(|agg| agg.task.id)
            .collect();
        drop(tasks);

        let active_tangles: Vec<Tangle> = self
            .tangles
            .lock()
            .unwrap()
            .values()
            .filter(|t| t.is_active())
            .cloned()
            .collect();
        let tangled_task_ids: std::collections::BTreeSet<TaskId> = active_tangles
            .iter()
            .flat_map(|t| t.task_ids.iter().copied())
            .collect();

        Ok(BlockingGraph {
            edges,
            done_task_ids,
            tangled_task_ids,
            tangles: active_tangles,
        })
    }
}
