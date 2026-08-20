import ProjectSection from '@/components/ProjectSection';
import ScriptsPanel from '@/components/ScriptsPanel';
import { PageBody } from '@/ui';

export default function ScriptsPage() {
  return (
    <PageBody>
      <ProjectSection
        permission="SCRIPT_READ"
        writeBit="SCRIPT_WRITE"
        render={(project, write) => (
          <ScriptsPanel project={project} write={write} />
        )}
      />
    </PageBody>
  );
}
