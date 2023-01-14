import { bundle } from 'src/protobufs/shared/bundle';

export class FileDto {
  /**
   * ID of the revision this file resides in
   */
  revisionId: string;

  /**
   * Name of the file
   */
  fileName: string;

  /**
   * Path of file relative from the root path
   */
  filePath: string;

  /**
   * Content of the file, utf-8 encoded
   */
  content: Uint8Array;

  constructor(file: bundle.File) {
    return Object.assign(this, file);
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

  constructor(bundle: bundle.Bundle) {
    this.entryPoint = bundle.entryPoint;
    this.files = bundle.files.map((file) => new FileDto(file));
  }
}
