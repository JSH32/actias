import { BadRequestException } from '@nestjs/common';

/**
 * Kv namespaces starting with this prefix belong to the platform; the kv
 * api neither lists nor serves them. Mirrors
 * `actias-common::naming::RESERVED_NAMESPACE_PREFIX`.
 */
export const RESERVED_NAMESPACE_PREFIX = '__';

/**
 * Rejects requests addressing a reserved namespace, so platform data (the
 * encrypted secrets among it) is unreachable through the kv api.
 */
export function assertNotReserved(namespace: string) {
  if (namespace.startsWith(RESERVED_NAMESPACE_PREFIX)) {
    throw new BadRequestException(
      `Namespace '${namespace}' is reserved for the platform.`,
    );
  }
}
