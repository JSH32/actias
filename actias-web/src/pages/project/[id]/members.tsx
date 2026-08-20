import AccessPanel from '@/components/AccessPanel';
import ProjectSection from '@/components/ProjectSection';
import { PageBody } from '@/ui';

export default function MembersPage() {
  return (
    <PageBody>
      <ProjectSection
        permission="PERMISSIONS_READ"
        writeBit="PERMISSIONS_WRITE"
        render={(project, write) => (
          <AccessPanel project={project} write={write} />
        )}
      />
    </PageBody>
  );
}
