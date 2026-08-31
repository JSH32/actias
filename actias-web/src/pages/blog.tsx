import Link from 'next/link';
import type { GetStaticProps } from 'next';
import { PostMeta, getAllPosts } from '@/helpers/blog';
import classes from './blog.module.css';

/** The date as the post header shows it, so the two agree. */
function published(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : date.toISOString().slice(0, 10);
}

function Blog({ posts }: { posts: PostMeta[] }) {
  return (
    <div className={classes.page}>
      <h1 className={classes.title}>Blog</h1>
      <p className={classes.lead}>
        What shipped, what broke, and what the platform can do that it could not
        before.
      </p>

      {posts.length === 0 ? (
        <p className={classes.empty}>Nothing written down yet.</p>
      ) : (
        <div className={classes.list}>
          {posts.map((post) => (
            <Link
              key={post.slug}
              href={`/posts/${post.slug}`}
              className={classes.post}
            >
              <div className={classes.meta}>
                {post.category && (
                  <span className={classes.category}>{post.category}</span>
                )}
                <span>{published(post.date)}</span>
              </div>
              <div className={classes.postTitle}>{post.title}</div>
              <p className={classes.excerpt}>{post.excerpt}</p>
            </Link>
          ))}
        </div>
      )}
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
