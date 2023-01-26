-- All types of resources that can be owned/accessed.
CREATE TYPE resource_type AS ENUM ('script');

-- Users table, stores user sign in data.
CREATE TABLE users
(
    id      UUID         NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    created TIMESTAMPTZ  NOT NULL DEFAULT now(),
    email   VARCHAR(320) NOT NULL UNIQUE,
    admin   BOOLEAN      NOT NULL DEFAULT false, -- System admin.

    PRIMARY KEY (id)
);

CREATE TYPE auth_method AS ENUM (
    'password',
    'google',
    'github'
);

CREATE TABLE user_auth_methods
(
    id              UUID        NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    user_id         UUID        NOT NULL,
    cached_username TEXT        NOT NULL,
    auth_method     auth_method NOT NULL,
    value           TEXT        NOT NULL,
    last_accessed   TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (auth_method, user_id), -- Only one auth method of the same type per user.
    PRIMARY KEY (id)
);

-- Projects which contain resources
CREATE TABLE projects
(
    id          UUID         NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    name        VARCHAR(32)  NOT NULL,
    owner_id    UUID         NOT NULL,

    PRIMARY KEY (id)
);

-- Default permissions per project.
CREATE TABLE default_permissions
(
    id              UUID     NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    project_id      UUID     NOT NULL UNIQUE,
    script_bitfield smallint NOT NULL,

    PRIMARY KEY (id),
    FOREIGN KEY (project_id) REFERENCES projects (id) ON DELETE CASCADE
);

-- Individual resources which exist within a project.
CREATE TABLE resources
(
    resource_type resource_type NOT NULL,
    resource_id   UUID          NOT NULL,
    project_id    UUID          NOT NULL,

    PRIMARY KEY (resource_type, resource_id),
    FOREIGN KEY (project_id) REFERENCES projects (id) ON DELETE CASCADE
);

-- Access control table per user per resource type.
CREATE TABLE access
(
    user_id             UUID            NOT NULL,
    project_id          UUID            NOT NULL,
    resource_type       resource_type   NOT NULL,
    permission_bitfield smallint        NOT NULL,

    PRIMARY KEY (user_id, project_id, resource_type, permission_bitfield),
    FOREIGN KEY (project_id) REFERENCES projects (id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
);
