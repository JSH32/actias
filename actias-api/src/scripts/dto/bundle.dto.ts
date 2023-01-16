import { bundle } from 'src/protobufs/shared/bundle';

export class FileDto {
  /**
   * ID of the revision this file resides in.
   * This is empty when creating a revision or uploading a bundle.
   */
  revisionId?: string;

  /**
   * Name of the file
   */
  fileName: string;

  /**
   * Path of file relative from the root path
   */
  filePath: string;

  /**
   * Content of the file, base64 encoded
   */
  content: string;

  constructor(file: bundle.File) {
    this.revisionId = file.revisionId;
    this.fileName = file.fileName;
    this.filePath = file.filePath;
    this.content = Buffer.from(file.content).toString('base64');
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
    const bundle = Object.assign({}, this) as unknown as bundle.Bundle;

    bundle.files = bundle.files.map((file) => ({
      ...file,
      content: Buffer.from(file.content as any, 'base64'),
    }));

    return bundle;
  }
}
