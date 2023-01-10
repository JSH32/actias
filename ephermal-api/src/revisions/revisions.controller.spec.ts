import { Test, TestingModule } from '@nestjs/testing';
import { RevisionsController } from './revisions.controller';

describe('RevisionsController', () => {
  let controller: RevisionsController;

  beforeEach(async () => {
    const module: TestingModule = await Test.createTestingModule({
      controllers: [RevisionsController],
    }).compile();

    controller = module.get<RevisionsController>(RevisionsController);
  });

  it('should be defined', () => {
    expect(controller).toBeDefined();
  });
});
