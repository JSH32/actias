/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type WorkflowRunDto = {
    /**
     * The caller-supplied run id.
     */
    id: string;
    definition: string;
    /**
     * completed, cancelled, sleeping, awaiting, running or unstarted.
     */
    status: string;
    /**
     * The status detail: due times, awaited signal, reason.
     */
    detail: any;
    /**
     * Journal rows so far.
     */
    entries: number;
    startedAt?: number | null;
    updatedAt?: number | null;
};

