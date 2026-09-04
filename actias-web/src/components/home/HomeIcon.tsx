/**
 * The landing's icons, from lucide: one stroke weight and size across
 * the page, named by what they mean so the copy beside them reads the
 * same. The GitHub mark stays the design's own, since lucide carries no
 * brand glyphs.
 */
import * as React from 'react';
import {
  Activity,
  ArrowRight,
  BookOpen,
  Box,
  Check,
  ChevronDown,
  Clock,
  Code,
  Copy,
  Database,
  Download,
  Eye,
  Folder,
  Inbox,
  LogIn,
  Lock,
  Mail,
  Pause,
  Play,
  Radio,
  Target,
  Upload,
  Users,
  type LucideIcon,
} from 'lucide-react';
import { Icon } from '@/ui/icons';

export type HomeIconName =
  | 'arrowRight'
  | 'book'
  | 'broadcast'
  | 'check'
  | 'chevronDown'
  | 'clock'
  | 'copy'
  | 'databases'
  | 'download'
  | 'eye'
  | 'folder'
  | 'github'
  | 'kv'
  | 'lock'
  | 'login'
  | 'mail'
  | 'members'
  | 'overview'
  | 'pause'
  | 'play'
  | 'queues'
  | 'scripts'
  | 'target'
  | 'upload';

const GLYPHS: Record<Exclude<HomeIconName, 'github'>, LucideIcon> = {
  arrowRight: ArrowRight,
  book: BookOpen,
  broadcast: Radio,
  check: Check,
  chevronDown: ChevronDown,
  clock: Clock,
  copy: Copy,
  databases: Database,
  download: Download,
  eye: Eye,
  folder: Folder,
  kv: Box,
  lock: Lock,
  login: LogIn,
  mail: Mail,
  members: Users,
  overview: Activity,
  pause: Pause,
  play: Play,
  queues: Inbox,
  scripts: Code,
  target: Target,
  upload: Upload,
};

export function HomeIcon({
  name,
  size = 17,
}: {
  name: HomeIconName;
  size?: number;
}) {
  if (name === 'github') {
    return <Icon name="github" size={size} />;
  }
  const Glyph = GLYPHS[name];
  return <Glyph size={size} strokeWidth={1.7} />;
}
