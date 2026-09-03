/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type ConnectionDto = {
    /**
     * Node-local connection id.
     */
    id: string;
    /**
     * The declared connection class running the wire.
     */
    connectionClass: string;
    /**
     * The identity it speaks as, 'Class/name'.
     */
    identity: string;
    /**
     * 'inbound' (a client's upgrade) or 'outbound' (dialled by the project).
     */
    direction: string;
    /**
     * The far side's host, outbound only.
     */
    peer?: string | null;
    /**
     * The node holding the wire.
     */
    node: string;
    /**
     * The script whose revision opened it.
     */
    scriptId: string;
    /**
     * Unix milliseconds the wire opened.
     */
    openedAt: number;
    /**
     * 'new', 'warm' (holding a vm) or 'hibernated' (wire kept, vm dropped).
     */
    status: string;
    /**
     * Edges the connection holds right now.
     */
    follows: number;
};

