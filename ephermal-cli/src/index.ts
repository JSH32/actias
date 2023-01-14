import path from 'path'
import yargs from 'yargs'
import fs from 'fs'
import { hideBin } from 'yargs/helpers'
import chalk from 'chalk'
import glob from "glob"
import { z } from "zod"
import { ErrorMessageOptions, generateErrorMessage } from 'zod-error'
import inquirer from 'inquirer'
import axios from 'axios'
import { promisify } from 'util'

const API_URL = "http://localhost:3006"

// Ephermal project file
const projectSchema = z.object({
    // ID of the project, this will be null at first.
    // On the first upload this will be set.
    id: z.nullable(z.string()),
    // First file which will be executed in the bundle.
    entryPoint: z.string(),
    // List of all files which will be included in their bundle.
    // All paths are relative to the project file.
    includes: z.array(z.string()),
})

/**
 * Parse project into a proper message.
 * @param projectPath 
 * @returns 
 */
const parseProject = (projectPath: string) => {
    const projectString = path.join(projectPath, "project.json")

    let projectContents;
    try {
        projectContents = fs.readFileSync(projectString, 'utf-8');
    } catch (err) {
        if (err.code !== 'ENOENT') throw err;
        throw `❌ Invalid ${chalk.yellow("project.json")} format or missing.`
    }

    const project = projectSchema.safeParse(JSON.parse(projectContents));
    if (!project.success) {
        const errorMessage = generateErrorMessage((project as any).error.issues, options);

        throw `❌ Problem parsing ${chalk.yellow("project.json")}\n${errorMessage}`
    }

    const includes = project.data.includes.map(pattern => glob.sync(pattern, {
        cwd: projectPath
    }))
    .flat()
    .filter(i => i.replace(/^.\//, '') !== "project.json" && !fs.lstatSync(path.join(projectPath, i)).isDirectory())

    return [project.data, {
        ...project.data,
        includes,
    }]
}

const parseBundle = async (projectPath: string, project: typeof projectSchema._output) => {    
    const files = await Promise.all(project.includes.map(async (i) => ({
        fileName: i.substring(i.lastIndexOf('/')+1),
        filePath: i.replace(/^.\//, ''),
        content: (await promisify(fs.readFile)(path.join(projectPath, i))).toJSON().data,
    })))

    return {
        entryPoint: project.entryPoint,
        files
    }
}

const options: ErrorMessageOptions = {
    delimiter: {
      error: '\n',
    },
    transform: ({ errorMessage, index }) => `Error #${index + 1}: ${errorMessage}`,
};  

yargs(hideBin(process.argv))
    .scriptName(chalk.magenta('ephermal'))
    .usage('$0 <cmd> [args]')
    .command(chalk.green('init <name>'), '📜 Initialize a new sample project')
    .command('init <name>', false, yargs => (
        yargs.positional('name', { describe: 'name of project', type: "string" })
    ), argv => {
        const fullPath = path.join(process.cwd(), argv.name)

        if (!fs.existsSync(argv.name) || fs.readdirSync(argv.name).length === 0) {
            // Empty, copy files
            fs.cpSync(path.join(__dirname, "../template"), argv.name, { recursive: true })
            console.info(chalk.green(`📜 Project ${chalk.magenta(argv.name)} was created!`))
        } else {
            console.error(`❌ Specified directory "${chalk.yellow(fullPath)}" is not empty`)
        }
    })
    .command(chalk.green("publish <directory>"), '🚀 Publish a new revision of the project')
    .command('publish <directory>', false, yargs => (
        yargs.positional('directory', { describe: 'directory of project to publish', type: "string" })
    ), async (argv) => {
        const fullPath = path.join(process.cwd(), argv.directory)
        
        let originalFile, project
        try {
            [originalFile, project] = parseProject(fullPath)
        } catch (e) {
            console.error(chalk.red(e.toString()))
            return
        }

        let script
        if (project.id === null) {
            const answer = await inquirer.prompt([{
                name: "createProject",
                type: "confirm",
                message: "Project doesn't have an ID, would you like to create a new project?"
            }])

            if (!answer.createProject) {
                console.error("❌ Can't publish project without an ID")
                return
            }

            const projectName = await inquirer.prompt([{
                name: "projectName",
                message: "What would you like the public identifier to be?"
            }])

            try {
                script = (await axios.post(`${API_URL}/scripts`, {
                    publicIdentifier: projectName.projectName
                })).data
            } catch (err) {
                console.error(`❌ ${err.response.data.message}`)
                return
            }

            fs.writeFileSync(path.join(fullPath, "project.json"), JSON.stringify({
                ...originalFile,
                id: script.id
            }, null, 2))

            // Set the ID's since that was the only thing missing previously.
            originalFile.id = script.id
            project.id = script.id

            console.info(`📜 Script has been created ${chalk.gray(`(${script.id})`)}`)
        } else {
            try {
                script = (await axios.get(`${API_URL}/scripts?id=${project.id}`)).data
            } catch (err) {
                console.error(`❌ ${err.response.data.message}`)
                return
            }
        }

        const parsedBundle = await parseBundle(fullPath, project)
        
        axios.patch(`${API_URL}/scripts/${script.id}/revision`, {
            bundle: parsedBundle,
            projectConfig: originalFile
        })
        .then(() => console.log(`🚀 Project published to ${script.publicIdentifier} ${chalk.gray(`(${script.id})`)}`))
        .catch(err => console.error(`❌ Can't publish project: ${err.response.data.message}`))
    })
    .strict()
    .parse()
