/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type WorkflowJournalRowDto = {
    seq: number;
    at: number;
    /**
     * STARTED, INTENT, RESULT, TIMER, SIGNAL, CHILD, CANCEL, COMPLETED or AMBIENT.
     */
    kind: string;
    data: any;
    format: number;
};

