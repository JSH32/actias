import KvPanel from '@/components/KvPanel';
import ProjectSection from '@/components/ProjectSection';

export default function KvPage() {
  return (
    <ProjectSection
      permission="KV_READ"
      writeBit="KV_WRITE"
      render={(project, write) => <KvPanel project={project} write={write} />}
    />
  );
}
