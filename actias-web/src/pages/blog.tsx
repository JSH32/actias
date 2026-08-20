import Link from 'next/link';
import type { GetStaticProps } from 'next';
import { PostMeta, getAllPosts } from '@/helpers/blog';
import { Card } from '@/ui';

function Blog({ posts }: { posts: PostMeta[] }) {
  return (
    <div style={{ maxWidth: 640, margin: '48px auto', padding: '0 24px' }}>
      <h1 style={{ fontSize: 20, fontWeight: 700, marginBottom: 16 }}>Blog</h1>
      {posts.map((post) => (
        <Link key={post.slug} href={`/posts/${post.slug}`}>
          <Card style={{ padding: 16, marginBottom: 10 }}>
            <div
              style={{
                fontFamily: 'var(--mono)',
                fontSize: 11,
                color: 'var(--luna)',
                textTransform: 'uppercase',
                letterSpacing: '0.08em',
              }}
            >
              {post.category}
            </div>
            <div style={{ fontWeight: 700, margin: '4px 0' }}>{post.title}</div>
            <p style={{ color: 'var(--ink-2)', fontSize: 13 }}>
              {post.excerpt}
            </p>
          </Card>
        </Link>
      ))}
    </div>
  );
}

export const getStaticProps: GetStaticProps = async () => {
  const posts = getAllPosts()
    .slice(0, 20)
    .map((post) => post.meta);
  return { props: { posts } };
};

export default Blog;
