import type { NextApiRequest, NextApiResponse } from 'next';

/** The probe target: the app serves, nothing deeper to ask of it. */
export default function handler(_req: NextApiRequest, res: NextApiResponse) {
  res.status(200).json({ status: 'ok' });
}
