WITH bounds AS (SELECT $2::text::date::timestamp AT TIME ZONE $3 AS start_at,($2::text::date+1)::timestamp AT TIME ZONE $3 AS end_at),
commits AS (SELECT DISTINCT ON (g.project_id,g.commit_sha) g.* FROM git_commit_authorship g JOIN project_repositories r ON r.id=g.repository_id JOIN git_connections c ON c.id=r.connection_id JOIN actors owner ON owner.id=c.owner_actor_id CROSS JOIN bounds b WHERE g.actor_id=$1 AND g.occurred_at>=b.start_at AND g.occurred_at<b.end_at AND r.authorization_status='authorized' AND NOT r.archived AND c.status='active' AND owner.active ORDER BY g.project_id,g.commit_sha,g.repository_id),
items AS (
 SELECT 'work_log' kind,w.id,left(coalesce(w.title,t.title,'Work'),160) title,concat(w.minutes,' minutes; ',coalesce(w.note,'')) detail,w.date::timestamp AT TIME ZONE $3 AS occurred_at,t.project_id,NULL::text repository_id FROM task_work_logs w LEFT JOIN tasks t ON t.id=w.task_id WHERE w.actor_id=$1 AND w.date=$2::text::date AND w.removed_at IS NULL
 UNION ALL
 SELECT 'task' kind,h.id,left(t.title,160) title,left(h.operation,80) detail,h.created_at occurred_at,h.project_id,NULL::text repository_id
 FROM task_history h JOIN tasks t ON t.id=h.task_id CROSS JOIN bounds b
 WHERE h.actor_id=$1 AND h.created_at>=b.start_at AND h.created_at<b.end_at
 UNION ALL
 SELECT 'commit',g.commit_sha,left(coalesce(g.commit_message,''),160),left(coalesce(g.commit_message,''),480),g.occurred_at,g.project_id,g.repository_id
 FROM commits g
 UNION ALL
 SELECT 'knowledge',v.id,left(v.title,160),left(v.content::text,480),v.created_at,v.project_id,NULL::text
 FROM project_document_versions v CROSS JOIN bounds b WHERE v.created_by=$1 AND v.created_at>=b.start_at AND v.created_at<b.end_at
 UNION ALL
 SELECT 'knowledge',v.document_id||':'||v.revision,left(v.title,160),left(v.body,480),v.created_at,NULL::text,NULL::text
 FROM organization_document_versions v CROSS JOIN bounds b WHERE v.created_by=$1 AND v.created_at>=b.start_at AND v.created_at<b.end_at
 UNION ALL
 SELECT 'knowledge',coalesce(e.metadata->>'knowledge_id',e.id),left(e.operation,160),'Repository '||coalesce(e.resource_id,'')||'; contribution '||coalesce(e.metadata->>'knowledge_id','')||'; revision '||coalesce(e.metadata->>'revision',''),e.created_at,e.metadata->>'project_id',e.resource_id
 FROM audit_events e CROSS JOIN bounds b WHERE e.actor_id=$1 AND e.operation LIKE 'knowledge.%' AND e.created_at>=b.start_at AND e.created_at<b.end_at AND EXISTS(SELECT 1 FROM project_repositories r JOIN git_connections c ON c.id=r.connection_id JOIN actors owner ON owner.id=c.owner_actor_id WHERE r.id=e.resource_id AND r.authorization_status='authorized' AND NOT r.archived AND c.status='active' AND owner.active)
), ranked AS (SELECT *,row_number() OVER(PARTITION BY kind ORDER BY occurred_at,id) position FROM items), limited AS (SELECT kind,id,title,detail,occurred_at,project_id,repository_id FROM ranked WHERE position<=8 ORDER BY kind,occurred_at,id)
SELECT jsonb_build_object('repository_ids',coalesce((SELECT jsonb_agg(DISTINCT repository_id) FILTER(WHERE repository_id IS NOT NULL) FROM items),'[]'::jsonb),'project_ids',coalesce((SELECT jsonb_agg(DISTINCT project_id) FILTER(WHERE project_id IS NOT NULL) FROM items),'[]'::jsonb),'items',coalesce((SELECT jsonb_agg(to_jsonb(limited)) FROM limited),'[]'::jsonb),'counts',(SELECT coalesce(jsonb_object_agg(kind,total),'{}'::jsonb) FROM (SELECT kind,count(*) total FROM items GROUP BY kind) counts))
