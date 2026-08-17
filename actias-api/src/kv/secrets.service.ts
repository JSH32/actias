import { Injectable, ServiceUnavailableException } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import * as crypto from 'crypto';

/**
 * Encrypts project secrets before they are stored.
 *
 * The stored form is `base64(nonce || ciphertext || tag)` under AES-256-GCM;
 * the worker decrypts with the same `SECRET_ENCRYPTION_KEY` (its
 * implementation carries a cross-language test vector pinning this format).
 */
@Injectable()
export class SecretsService {
  private readonly key?: Buffer;

  constructor(config: ConfigService) {
    const encoded = config.get<string>('secretEncryptionKey');
    if (encoded) {
      const key = Buffer.from(encoded, 'base64');
      if (key.length !== 32) {
        throw new Error(
          'SECRET_ENCRYPTION_KEY must decode to exactly 32 bytes',
        );
      }
      this.key = key;
    }
  }

  /**
   * Encrypts one secret value for storage.
   */
  encrypt(plaintext: string): string {
    if (!this.key) {
      throw new ServiceUnavailableException(
        'Secrets are not configured on this deployment.',
      );
    }

    const nonce = crypto.randomBytes(12);
    const cipher = crypto.createCipheriv('aes-256-gcm', this.key, nonce);
    const ciphertext = Buffer.concat([
      cipher.update(plaintext, 'utf8'),
      cipher.final(),
    ]);

    return Buffer.concat([nonce, ciphertext, cipher.getAuthTag()]).toString(
      'base64',
    );
  }
}
