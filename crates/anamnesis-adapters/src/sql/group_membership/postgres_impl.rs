use super::*;

pub(super) async fn is_system_admin_via_group(
    pool: &PgPool,
    user: &UserId,
) -> Result<bool, RepoError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT ug.group_name FROM user_groups ug \
         JOIN system_admin_groups sag ON sag.group_name = ug.group_name \
         WHERE ug.user_id = $1 LIMIT 1",
    )
    .bind(user.as_str())
    .fetch_optional(pool)
    .await
    .map_err(|e| RepoError::from_source("failed to check system admin group", e))?;
    Ok(row.is_some())
}

pub(super) async fn area_group_role(
    pool: &PgPool,
    user: &UserId,
    area: AreaId,
) -> Result<Option<Role>, RepoError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT agm.role FROM user_groups ug \
         JOIN area_group_members agm ON agm.group_name = ug.group_name \
         WHERE ug.user_id = $1 AND agm.area_id = $2",
    )
    .bind(user.as_str())
    .bind(area.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| RepoError::from_source("failed to load area group role", e))?;
    strongest_role(rows.into_iter().map(|(r,)| r).collect())
}

pub(super) async fn project_group_role(
    pool: &PgPool,
    user: &UserId,
    project: ProjectId,
) -> Result<Option<Role>, RepoError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT pgm.role FROM user_groups ug \
         JOIN project_group_members pgm ON pgm.group_name = ug.group_name \
         WHERE ug.user_id = $1 AND pgm.project_id = $2",
    )
    .bind(user.as_str())
    .bind(project.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| RepoError::from_source("failed to load project group role", e))?;
    strongest_role(rows.into_iter().map(|(r,)| r).collect())
}

pub(super) async fn list_admin_groups(pool: &PgPool) -> Result<Vec<String>, RepoError> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT group_name FROM system_admin_groups ORDER BY group_name")
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list admin groups", e))?;
    Ok(rows.into_iter().map(|(g,)| g).collect())
}

pub(super) async fn list_area_groups(
    pool: &PgPool,
    area: AreaId,
) -> Result<Vec<(String, Role)>, RepoError> {
    let rows = sqlx::query(
        "SELECT group_name, role FROM area_group_members WHERE area_id = $1 \
         ORDER BY group_name",
    )
    .bind(area.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| RepoError::from_source("failed to list area groups", e))?;
    group_role_pairs(rows)
}

pub(super) async fn list_project_groups(
    pool: &PgPool,
    project: ProjectId,
) -> Result<Vec<(String, Role)>, RepoError> {
    let rows = sqlx::query(
        "SELECT group_name, role FROM project_group_members WHERE project_id = $1 \
         ORDER BY group_name",
    )
    .bind(project.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| RepoError::from_source("failed to list project groups", e))?;
    group_role_pairs(rows)
}

pub(super) async fn list_known_groups(pool: &PgPool) -> Result<Vec<String>, RepoError> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT DISTINCT group_name FROM user_groups ORDER BY group_name")
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list known groups", e))?;
    Ok(rows.into_iter().map(|(g,)| g).collect())
}

pub(super) async fn replace_user_groups(
    pool: &PgPool,
    user: &UserId,
    groups: &[String],
) -> Result<(), RepoError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| RepoError::from_source("failed to begin group replacement", e))?;
    sqlx::query("DELETE FROM user_groups WHERE user_id = $1")
        .bind(user.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to clear user groups", e))?;
    for group in groups {
        sqlx::query(
            "INSERT INTO user_groups (user_id, group_name) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(user.as_str())
        .bind(group)
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to record user group", e))?;
    }
    tx.commit()
        .await
        .map_err(|e| RepoError::from_source("failed to commit group replacement", e))
}

pub(super) async fn grant_admin_group(pool: &PgPool, group: &str) -> Result<(), RepoError> {
    sqlx::query("INSERT INTO system_admin_groups (group_name) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(group)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to grant admin group", e))?;
    Ok(())
}

pub(super) async fn set_area_group_role(
    pool: &PgPool,
    group: &str,
    area: AreaId,
    role: Role,
) -> Result<(), RepoError> {
    sqlx::query(
        "INSERT INTO area_group_members (group_name, area_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT (group_name, area_id) DO UPDATE SET role = excluded.role",
    )
    .bind(group)
    .bind(area.as_uuid())
    .bind(role_to_text(role))
    .execute(pool)
    .await
    .map_err(|e| RepoError::from_source("failed to set area group role", e))?;
    Ok(())
}

pub(super) async fn set_project_group_role(
    pool: &PgPool,
    group: &str,
    project: ProjectId,
    role: Role,
) -> Result<(), RepoError> {
    sqlx::query(
        "INSERT INTO project_group_members (group_name, project_id, role) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (group_name, project_id) DO UPDATE SET role = excluded.role",
    )
    .bind(group)
    .bind(project.as_uuid())
    .bind(role_to_text(role))
    .execute(pool)
    .await
    .map_err(|e| RepoError::from_source("failed to set project group role", e))?;
    Ok(())
}

pub(super) async fn revoke_admin_group(pool: &PgPool, group: &str) -> Result<(), RepoError> {
    sqlx::query("DELETE FROM system_admin_groups WHERE group_name = $1")
        .bind(group)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to revoke admin group", e))?;
    Ok(())
}

pub(super) async fn revoke_area_group_role(
    pool: &PgPool,
    group: &str,
    area: AreaId,
) -> Result<(), RepoError> {
    sqlx::query("DELETE FROM area_group_members WHERE group_name = $1 AND area_id = $2")
        .bind(group)
        .bind(area.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to revoke area group role", e))?;
    Ok(())
}

pub(super) async fn revoke_project_group_role(
    pool: &PgPool,
    group: &str,
    project: ProjectId,
) -> Result<(), RepoError> {
    sqlx::query("DELETE FROM project_group_members WHERE group_name = $1 AND project_id = $2")
        .bind(group)
        .bind(project.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to revoke project group role", e))?;
    Ok(())
}
