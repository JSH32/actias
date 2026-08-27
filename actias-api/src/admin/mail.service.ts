import { Injectable } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import { Transporter, createTransport } from 'nodemailer';

/**
 * Outbound mail, entirely optional: SMTP_HOST being set is the switch.
 * Without it every caller sees `enabled` false and falls back to
 * something copyable; nothing in the platform requires mail to work.
 */
@Injectable()
export class MailService {
  private readonly transport: Transporter | null = null;
  private readonly from: string;

  constructor(config: ConfigService) {
    const smtp = config.get<{
      host?: string;
      port: number;
      user?: string;
      pass?: string;
      from?: string;
    }>('smtp');

    this.from = smtp.from || 'actias@localhost';
    if (smtp.host) {
      this.transport = createTransport({
        host: smtp.host,
        port: smtp.port,
        auth: smtp.user ? { user: smtp.user, pass: smtp.pass } : undefined,
      });
    }
  }

  get enabled(): boolean {
    return this.transport !== null;
  }

  /** Sends the invite; the caller already knows `enabled` is true. */
  async sendInvite(to: string, link: string): Promise<void> {
    if (!this.transport) throw new Error('SMTP is not configured.');
    await this.transport.sendMail({
      from: this.from,
      to,
      subject: 'An Actias account is waiting for you',
      text: `You have been invited to an Actias instance.\n\nRegister here: ${link}\n\nThe link carries a one-use registration code.`,
    });
  }
}
