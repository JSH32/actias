DROP INDEX object_instances_object_id;
DROP INDEX object_instances_expire_at;
ALTER TABLE object_instances DROP COLUMN created_by;
ALTER TABLE object_instances DROP COLUMN deleted_at;
ALTER TABLE object_instances DROP COLUMN expire_at;
ALTER TABLE object_instances DROP COLUMN object_id;
