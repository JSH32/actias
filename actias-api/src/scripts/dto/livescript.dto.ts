import { CreateRevisionDto } from './requests.dto';

/**
 * Intended to be used with Websocket.
 */
export class LiveScriptDto {
  /**
   * Script ID to create session for.
   */
  scriptId: string;
  /**
   * Session ID (empty when creating).
   */
  sessionId?: string;
  /**
   * Revision content
   */
  revision: CreateRevisionDto;
}
