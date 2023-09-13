import { Anchor } from '@mantine/core';
import Link from 'next/link';

interface BreadCrumbLink {
  title: string;
  href: string;
}

export const breadcrumbs = (items: BreadCrumbLink[]) =>
  items.map((item, index) => (
    <Link key={index} href={item?.href} passHref>
      <Anchor component="button">{item.title}</Anchor>
    </Link>
  ));
