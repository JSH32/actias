/* istanbul ignore file */
/* tslint:disable */

export type ProjectDto = {
    id: string;
    /**
     * Name of project.
     */
    name: string;
    /**
     * Owner of project, has full access.
     */
    ownerId: string;
    createdAt: string;
    updatedAt: string;
};

