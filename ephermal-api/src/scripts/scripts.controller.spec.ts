import { Test, TestingModule } from '@nestjs/testing';
import { ScriptsController } from './scripts.controller';

describe('ScriptsController', () => {
  let controller: ScriptsController;

  beforeEach(async () => {
    const module: TestingModule = await Test.createTestingModule({
      controllers: [ScriptsController],
    }).compile();

    controller = module.get<ScriptsController>(ScriptsController);
  });

  it('should be defined', () => {
    expect(controller).toBeDefined();
  });
});
