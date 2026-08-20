/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type WorkflowDefinitionDto = {
    name: string;
    /**
     * Public identifier of the declaring script.
     */
    declaredBy: string;
    /**
     * Step name literals found at publish: a superset of what may run, rendered as the hollow skeleton.
     */
    stepNames: Array<string>;
};

