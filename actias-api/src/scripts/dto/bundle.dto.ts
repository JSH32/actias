import { ApiProperty } from '@nestjs/swagger';
import { bundle } from 'src/protobufs/shared/bundle';

export class FileDto {
  /**
   * Path of the file relative to the bundle root; its identity within the
   * bundle.
   */
  filePath: string;

  /**
   * Content of the file, base64 encoded.
   */
  content: string;

  /**
   * Mime type served for assets; informative for modules.
   */
  contentType?: string;

  /**
   * How the platform treats the file; modules are lua source, assets are
   * served as-is. Defaults to module.
   */
  @ApiProperty({ enum: ['module', 'asset'], required: false })
  kind?: 'module' | 'asset';

  /**
   * blake3 of the content, computed by the store; ignored on upload.
   */
  hash?: string;

  /**
   * Content size in bytes, computed by the store; ignored on upload.
   */
  size?: number;

  constructor(file: bundle.File) {
    this.filePath = file.filePath;
    this.content = Buffer.from(file.content).toString('base64');
    this.contentType = file.contentType;
    this.kind =
      file.kind === bundle.FileKind.FILE_KIND_ASSET ? 'asset' : 'module';
    this.hash = file.hash;
    this.size = file.size;
  }
}

export class BundleDto {
  /**
   * Path of the entrypoint file.
   * This is the first file which is executed by the runtime.
   */
  entryPoint: string;

  /**
   * All files within the bundle.
   */
  files: FileDto[];

  constructor(bundle: Partial<BundleDto>) {
    Object.assign(this, bundle);
  }

  static fromServiceBundle(bundle: bundle.Bundle) {
    const bundleDto = new BundleDto(bundle as any);
    bundleDto.entryPoint = bundle.entryPoint;
    bundleDto.files = bundle.files.map((file) => new FileDto(file));
    return bundleDto;
  }

  toServiceBundle(): bundle.Bundle {
    const serviceBundle = Object.assign({}, this) as unknown as bundle.Bundle;

    serviceBundle.files = this.files.map(
      (file) =>
        ({
          filePath: file.filePath,
          content: Buffer.from(file.content as any, 'base64'),
          contentType: file.contentType ?? '',
          kind:
            file.kind === 'asset'
              ? bundle.FileKind.FILE_KIND_ASSET
              : bundle.FileKind.FILE_KIND_MODULE,
        } as bundle.File),
    );

    return serviceBundle;
  }
}
