import { useStore } from '@/helpers/state';
import { observer } from 'mobx-react-lite';
import { useRouter } from 'next/router';
import * as React from 'react';

export function withAuthentication(Component: React.FC<any>) {
  const AuthenticatedComponent: React.FC<any> = observer((props) => {
    const [isClient, setIsClient] = React.useState(false);
    const router = useRouter();

    const store = useStore();

    React.useEffect(() => {
      setIsClient(true);

      store?.fetchUserInfo().then(() => {
        if (!store?.userData) {
          router.push('/login');
        }
      });
    }, [store, router]);

    if (!store?.userData || !isClient) {
      return <></>;
    }

    return <Component {...props} />;
  });

  return AuthenticatedComponent;
}
