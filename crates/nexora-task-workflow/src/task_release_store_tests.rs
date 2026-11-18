use super::TaskReleaseStore;
#[allow(clippy::duplicate_mod)]
#[path = "../tests/support/task_pool_fixture.rs"]
mod task_pool_fixture;
#[allow(clippy::duplicate_mod)]
#[path = "../tests/support/task_test_sql.rs"]
mod test_sql;
use nexora_platform::*;
use serde_json::json;
use task_pool_fixture::TaskPoolFixture;
#[test]
#[ignore = "requires PostgreSQL; private publisher receives independently verified repository facts"]
fn selected_parent_publication_pins_integrated_children_and_completes_them_before_parent_atomically()
 {
    let mut f = TaskPoolFixture::new(false);
    let project = f.work.project.id.clone();
    f.work
        .store
        .create_catalog_task_type(
            &f.work.ctx,
            &CatalogTaskTypeCreate {
                code: "release_work".into(),
                name: "Release work".into(),
                purpose: TaskPurpose::Implementation,
                description: None,
                color: None,
                icon: TaskTypeIcon::default(),
            },
        )
        .unwrap();
    let parent = f
        .work
        .store
        .create_pool_task(
            &f.work.ctx,
            &PoolTaskCreate {
                project_id: Some(project.clone()),
                definition: TaskCreate {
                    title: "Selected parent".into(),
                    type_code: "release_work".into(),
                    assignee_id: Some(f.developer.actor_id().into()),
                    entry_workflow_id: Some(f.version.workflow_id.clone()),
                    origin: "test".into(),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    let mut sql = test_sql::connect(&f.work.config, &f.work.namespace);
    let parent = f
        .work
        .store
        .take_task(
            &f.developer,
            &parent.id,
            &TaskTake {
                expected_revision: parent.revision,
                workflow_id: None,
            },
            Some(&project),
        )
        .unwrap();
    let child = f
        .work
        .store
        .create_task_subtask(
            &f.developer,
            &parent.id,
            &TaskSubtaskCreate {
                expected_revision: parent.revision,
                work_kind: TaskSubtaskKind::Decomposition,
                tested_commit_sha: None,
                tested_repository_id: None,
                definition: TaskCreate {
                    title: "Integrated child".into(),
                    type_code: "release_work".into(),
                    assignee_id: Some(f.developer.actor_id().into()),
                    entry_workflow_id: Some(f.version.workflow_id.clone()),
                    origin: "test".into(),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    let child = f
        .work
        .store
        .take_task(
            &f.developer,
            &child.id,
            &TaskTake {
                expected_revision: child.revision,
                workflow_id: None,
            },
            Some(&project),
        )
        .unwrap();
    let child = f
        .work
        .store
        .transition_pool_task(
            &f.developer,
            &child.id,
            "finish",
            &TaskTransitionRequest {
                expected_revision: child.revision,
                ..Default::default()
            },
        )
        .unwrap();
    let parent = f.work.store.pool_task(&f.developer, &parent.id).unwrap();
    // Persistence-only fixture: provider integration is verified by the transition executor, outside this private publisher test.
    sql.execute(
        "UPDATE tasks SET state_id='done',revision=revision+1 WHERE id=$1",
        &[&parent.id],
    )
    .unwrap();
    let parent = f.work.store.pool_task(&f.developer, &parent.id).unwrap();
    sql.execute("INSERT INTO git_connections(id,provider,host,owner_actor_id,remote_account_id,remote_login,status,config_revision,tokens,expires_at) VALUES('cascade-connection','gitlab','fixture.local',$1,'1','fixture','active',1,'{}',now()+interval '1 hour')",&[&f.work.ctx.actor_id()]).unwrap();
    sql.execute("INSERT INTO project_repositories(id,project_id,provider,host,requested_full_name,remote_id,remote_full_name,connection_id,authorization_status) VALUES('repo_cascade',$1,'gitlab','fixture.local','fixture/cascade','1','fixture/cascade','cascade-connection','authorized')",&[&project]).unwrap();
    let source = "1".repeat(40);
    let head = "2".repeat(40);
    for task in [&parent, &child] {
        sql.execute("INSERT INTO task_repository_branches(task_id,repository_id,branch_name,base_branch,base_sha,head_sha,evidence) VALUES($1,'repo_cascade',$1,'develop',$2,$2,'{\"provenance\":\"provider_verified\"}')",&[&task.id,&source]).unwrap();
    }
    for task in [&parent, &child] {
        let link = format!("link-{}", task.id);
        sql.execute("INSERT INTO task_commit_links(id,task_id,repository_id,commit_sha,sequence,provider_evidence,actor_id) VALUES($1,$2,'repo_cascade',$3,1,'{\"provenance\":\"provider_verified\"}',$4)",&[&link,&task.id,&source,&f.developer.actor_id()]).unwrap();
    }
    let release=f.work.store.save_task_release(&f.manager,&project,None,&json!({"version":"cascade","branch_name":"release/cascade","tasks":[{"task_id":parent.id,"expected_revision":parent.revision}],"repositories":[{"repository_id":"repo_cascade","head_sha":head}]})).unwrap();
    assert_eq!(release["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(release["sources"].as_array().unwrap().len(), 2);
    let id = release["id"].as_str().unwrap();
    sql.execute("UPDATE task_release_repositories SET composition_status='composed',composition_evidence=$2 WHERE release_id=$1",&[&id,&json!({"head_sha":head,"base_sha":source,"commits":[source],"branch_name":"release/cascade","provenance":"server_composed"})]).unwrap();
    let run = json!({"id":"private-unit-provider-run","advanced":false});
    let mut selection = TaskTransitionRequest {
        expected_revision: parent.revision,
        release_id: Some(id.into()),
        release_revision: Some(1),
        ..Default::default()
    };
    assert!(matches!(
        crate::task_release_publication::TaskReleasePublication::selection(
            &mut f.work.store,
            &f.manager,
            Some(&project),
            &selection,
            &run
        ),
        Err(PlatformError::Validation(_))
    ));
    selection.branches = vec![TaskTransitionBranch {
        repository_id: "repo_cascade".into(),
        branch_name: "release/cascade".into(),
        expected_head_sha: head.clone(),
    }];
    crate::task_release_publication::TaskReleasePublication::selection(
        &mut f.work.store,
        &f.other,
        Some(&project),
        &selection,
        &run,
    )
    .unwrap();
    assert!(matches!(
        crate::task_release_publication::TaskReleasePublication::selection(
            &mut f.work.store,
            &f.developer,
            Some(&project),
            &selection,
            &run
        ),
        Err(PlatformError::Forbidden)
    ));
    let fingerprint =
        crate::task_approval_fingerprint::TaskApprovalFingerprint::parameters(&selection).unwrap();
    assert_eq!(fingerprint["release_id"], id);
    assert_eq!(fingerprint["release_revision"], 1);
    selection.release_revision = Some(2);
    assert!(matches!(
        crate::task_release_publication::TaskReleasePublication::selection(
            &mut f.work.store,
            &f.other,
            Some(&project),
            &selection,
            &run
        ),
        Err(PlatformError::Conflict(_))
    ));
    assert_ne!(
        fingerprint,
        crate::task_approval_fingerprint::TaskApprovalFingerprint::parameters(&selection).unwrap()
    );
    let evidence =
        json!({"transition_run_id":"private-unit-provider-run","provenance":"provider_verified"});
    assert!(matches!(
        TaskReleaseStore::publish(
            &mut f.work.store,
            &f.manager,
            &project,
            id,
            1,
            &[("repo_cascade".into(), source.clone())],
            &evidence
        ),
        Err(PlatformError::Conflict(_))
    ));
    sql.execute(
        "UPDATE task_repository_branches SET head_sha=$2 WHERE task_id=$1",
        &[&child.id, &head],
    )
    .unwrap();
    assert!(matches!(
        TaskReleaseStore::publish(
            &mut f.work.store,
            &f.manager,
            &project,
            id,
            1,
            &[("repo_cascade".into(), head.clone())],
            &evidence
        ),
        Err(PlatformError::Conflict(_))
    ));
    assert_eq!(
        f.work
            .store
            .pool_task(&f.developer, &parent.id)
            .unwrap()
            .lifecycle,
        TaskLifecycle::Active
    );
    sql.execute(
        "UPDATE task_repository_branches SET head_sha=$2 WHERE task_id=$1",
        &[&child.id, &source],
    )
    .unwrap();
    let published = TaskReleaseStore::publish(
        &mut f.work.store,
        &f.other,
        &project,
        id,
        1,
        &[("repo_cascade".into(), head.clone())],
        &evidence,
    )
    .unwrap();
    assert_eq!(published["status"], "published");
    for task in [&parent, &child] {
        assert_eq!(
            f.work
                .store
                .pool_task(&f.developer, &task.id)
                .unwrap()
                .lifecycle,
            TaskLifecycle::Completed
        );
    }
    let replay = TaskReleaseStore::publish(
        &mut f.work.store,
        &f.other,
        &project,
        id,
        1,
        &[("repo_cascade".into(), head)],
        &evidence,
    )
    .unwrap();
    assert_eq!(replay["revision"], 2);
    let count: i64 = sql
        .query_one(
            "SELECT count(*) FROM task_production_publications WHERE release_id=$1",
            &[&id],
        )
        .unwrap()
        .get(0);
    assert_eq!(count, 2);
}
