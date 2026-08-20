import ProjectSection from '@/components/ProjectSection';
import ScriptsPanel from '@/components/ScriptsPanel';

export default function ScriptsPage() {
  return (
    <ProjectSection
      permission="SCRIPT_READ"
      writeBit="SCRIPT_WRITE"
      render={(project, write) => (
        <ScriptsPanel project={project} write={write} />
      )}
    />
  );
}
