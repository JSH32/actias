DROP TABLE project_moves;
ALTER TABLE project_policies
    DROP COLUMN moving,
    DROP COLUMN region;
DROP TABLE regions;
