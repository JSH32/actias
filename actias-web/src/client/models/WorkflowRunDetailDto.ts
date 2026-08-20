/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { WorkflowJournalRowDto } from './WorkflowJournalRowDto';

export type WorkflowRunDetailDto = {
    id: string;
    definition: string;
    status: string;
    detail: any;
    journal: Array<WorkflowJournalRowDto>;
};

