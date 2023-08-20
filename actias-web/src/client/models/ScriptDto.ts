/* istanbul ignore file */
/* tslint:disable */

export type ScriptDto = {
    id: string;
    /**
     * Public identifier of the script.
     * This must be globally unique
     */
    publicIdentifier: string;
    /**
     * Date that the script (or revisions) were last updated.
     */
    lastUpdated: string;
    /**
     * ID of the currently deployed revision.
     * This is empty for newly created scripts.
     */
    currentRevisionId?: string;
    /**
     * Parent project that owns this script.
     */
    projectId: string;
};

