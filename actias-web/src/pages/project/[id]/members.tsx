import AccessPanel from '@/components/AccessPanel';
import ProjectSection from '@/components/ProjectSection';

export default function MembersPage() {
  return (
    <ProjectSection
      permission="PERMISSIONS_READ"
      writeBit="PERMISSIONS_WRITE"
      render={(project, write) => (
        <AccessPanel project={project} write={write} />
      )}
    />
  );
}
