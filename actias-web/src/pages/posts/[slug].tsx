/* eslint-disable react/jsx-props-no-spreading */
import React from 'react';

import type { GetStaticPaths, GetStaticProps } from 'next';
import { MDXRemote, MDXRemoteSerializeResult } from 'next-mdx-remote';
import { serialize } from 'next-mdx-remote/serialize';
import { NextSeo } from 'next-seo';
import Link from 'next/link';

import type { MDXComponents } from 'mdx/types';
import rehypeAutolinkHeadings from 'rehype-autolink-headings';
import rehypeSlug from 'rehype-slug';

import { PostMeta, getPostFromSlug, getSlugs } from '@/helpers/blog';

interface MDXPost {
  source: MDXRemoteSerializeResult<Record<string, unknown>>;
  meta: PostMeta;
}

/** Prose renders in Newsreader per the type sheet; code stays mono. */
const codeBlock: React.CSSProperties = {
  background: 'var(--night-2)',
  border: '1px solid var(--line)',
  borderRadius: 'var(--r2)',
  fontFamily: 'var(--mono)',
  fontSize: 13,
  padding: '12px 14px',
  overflowX: 'auto',
  margin: '12px 0',
};

const heading =
  (size: number) =>
  // eslint-disable-next-line react/display-name
  (props: object) => (
    <h2
      style={{
        fontFamily: 'var(--ui)',
        fontSize: size,
        fontWeight: 700,
        margin: '24px 0 8px',
      }}
      {...props}
    />
  );

const components: MDXComponents = {
  img: (props: { src?: string; alt?: string }) => (
    // eslint-disable-next-line @next/next/no-img-element
    <img
      style={{ objectFit: 'cover', maxWidth: '100%' }}
      src={props.src}
      alt={props.alt}
    />
  ),
  code: (props: object) => (
    <code
      style={{
        fontFamily: 'var(--mono)',
        fontSize: '0.9em',
        color: 'var(--kind-db)',
      }}
      {...props}
    />
  ),
  pre: (props: React.HTMLAttributes<HTMLPreElement>) => {
    const child = props.children as
      | React.ReactElement<{ children?: string }>
      | undefined;
    return <pre style={codeBlock}>{child?.props?.children}</pre>;
  },
  h1: heading(24),
  h2: heading(20),
  h3: heading(17),
  h4: heading(15),
  h5: heading(14),
  h6: heading(13),
};

export default function PostPage({ post }: { post: MDXPost }) {
  return (
    <div
      style={{
        maxWidth: 680,
        margin: '32px auto',
        padding: '0 24px',
        fontFamily: 'var(--prose)',
        fontSize: 16,
        lineHeight: 1.7,
      }}
    >
      <NextSeo title={post.meta.title} description={post.meta.excerpt} />
      <Link
        href="/blog"
        style={{
          fontFamily: 'var(--mono)',
          fontSize: 12,
          color: 'var(--ink-3)',
        }}
      >
        ← blog
      </Link>
      <h1
        style={{
          fontFamily: 'var(--ui)',
          fontSize: 26,
          fontWeight: 700,
          margin: '8px 0 4px',
        }}
      >
        {post.meta.title}
      </h1>
      <hr
        style={{
          border: 'none',
          borderTop: '1px solid var(--line)',
          margin: '12px 0 20px',
        }}
      />
      <MDXRemote {...post.source} components={components} />
    </div>
  );
}

export const getStaticProps: GetStaticProps = async ({ params }) => {
  const { slug } = params as { slug: string };
  const { content, meta } = getPostFromSlug(slug);
  const mdxSource = await serialize(content, {
    mdxOptions: {
      rehypePlugins: [
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
