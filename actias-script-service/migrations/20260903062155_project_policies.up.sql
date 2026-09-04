-- A project's runtime policy: what its scripts may spend on a node and
-- where they may reach. Set through the api, read by every worker on
-- the pointer ttl. A project with no row runs on the platform defaults,
-- which the zeros spell: unbounded rates, and every host the node's own
-- egress policy allows.
CREATE TABLE project_policies (
    project_id UUID PRIMARY KEY,
    -- Requests a node admits per second, burst of the same size; 0 is
    -- unbounded.
    requests_per_sec INTEGER NOT NULL DEFAULT 0,
    -- Work units a node lets the project spend per second; 0 is
    -- unbounded.
    work_units_per_sec BIGINT NOT NULL DEFAULT 0,
    -- Hosts outbound requests may reach (a leading dot matches
    -- subdomains); empty admits everything not denied.
    egress_allow TEXT[] NOT NULL DEFAULT '{}',
    -- Hosts refused before the allow list is consulted.
    egress_deny TEXT[] NOT NULL DEFAULT '{}',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
