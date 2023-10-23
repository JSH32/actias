/* eslint-disable react/jsx-props-no-spreading */
import React from 'react';

import type { GetStaticPaths, GetStaticProps } from 'next';
import { MDXRemote, MDXRemoteSerializeResult } from 'next-mdx-remote';
import { serialize } from 'next-mdx-remote/serialize';
import { NextSeo } from 'next-seo';
import Image from 'next/image';
import Link from 'next/link';

import {
  ActionIcon,
  Anchor,
  Code,
  Group,
  Table as ReactTable,
  Stack,
  Title,
} from '@mantine/core';
import { Prism } from '@mantine/prism';

import { IconArrowNarrowLeft } from '@tabler/icons-react';

import type { MDXComponents } from 'mdx/types';
import rehypeAutolinkHeadings from 'rehype-autolink-headings';
import rehypeSlug from 'rehype-slug';

import { PostMeta, getPostFromSlug, getSlugs } from '@/helpers/blog';

interface MDXPost {
  source: MDXRemoteSerializeResult<Record<string, unknown>>;
  meta: PostMeta;
}

const components: MDXComponents = {
  Image,
  ReactTable,
  code: (props: any) => <Code {...props} />,
  pre: (props: any) => {
    const matches = (props.children.props.className || '').match(
      /language-(?<lang>.*)/,
    );

    return (
      <Stack>
        <Prism
          language={
            matches && matches.groups && matches.groups.lang
              ? matches.groups.lang
              : ''
          }
        >
          {props.children.props.children}
        </Prism>
      </Stack>
    );
  },
  h1: (props: any) => <Title order={1} {...props} />,
  h2: (props: any) => <Title order={2} {...props} />,
  h3: (props: any) => <Title order={3} {...props} />,
  h4: (props: any) => <Title order={4} {...props} />,
  h5: (props: any) => <Title order={5} {...props} />,
  h6: (props: any) => <Title order={6} {...props} />,
  a: (props: any) => <Anchor {...props} />,
};

export default function PostPage({ post }: { post: MDXPost }) {
  return (
    <>
      <NextSeo title={post.meta.title} description={post.meta.excerpt} />
      <Group>
        <Link href={'/blog'} passHref>
          <ActionIcon aria-label="back to blog list">
            <IconArrowNarrowLeft size={34} />
          </ActionIcon>
        </Link>
        <Title order={1}>{post.meta.title}</Title>
      </Group>

      <MDXRemote {...post.source} components={components} />
    </>
  );
}

export const getStaticProps: GetStaticProps = async ({ params }) => {
  const { slug } = params as { slug: string };
  const { content, meta } = getPostFromSlug(slug);
  const mdxSource = await serialize(content, {
    mdxOptions: {
      rehypePlugins: [
        // rehypeHighlight,
        rehypeSlug,
        [rehypeAutolinkHeadings, { behavior: 'wrap' }],
      ],
    },
  });

  return { props: { post: { source: mdxSource, meta } } };
};

export const getStaticPaths: GetStaticPaths = async () => {
  const paths = getSlugs().map((slug) => ({ params: { slug } }));

  return {
    paths,
    fallback: false,
  };
};
